//! Song detection scheduler — periodically identifies the currently recording
//! audio via the Shazam API and exposes the result for display.

use crate::shazam::{RecognizeResult, Shazam};
use crate::vu_meter::SampleFormat;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

/// Accumulates raw audio and periodically runs Shazam recognition in a
/// background thread.
pub struct SongDetectScheduler {
    /// How often to attempt recognition (seconds).
    interval_secs: f64,
    /// Sample rate of the incoming audio.
    source_rate: u32,
    /// Number of channels in the incoming audio.
    _source_channels: usize,
    /// Sample format of the incoming audio.
    source_format: SampleFormat,
    /// Ring-buffer of 16-bit 16 kHz mono samples (latest ~15 seconds).
    pcm_buf: Arc<Mutex<Vec<i16>>>,
    /// Maximum number of samples to keep (16 kHz × 15 s).
    max_pcm_samples: usize,
    /// Last time a detection was launched.
    last_detect: Instant,
    /// Latest detection result, shared with the display thread.
    result: Arc<Mutex<Option<RecognizeResult>>>,
    /// Whether a detection is currently in progress.
    detecting: Arc<Mutex<bool>>,
    /// Whether a detection has ever been attempted.
    ever_attempted: Arc<Mutex<bool>>,
}

impl SongDetectScheduler {
    /// Create a new scheduler.
    ///
    /// * `interval_secs` — how often to run detection (e.g. 180.0 for 3 min)
    /// * `source_rate`   — sample rate of the audio being fed
    /// * `source_channels` — number of channels
    /// * `source_format` — S16 or S32
    pub fn new(
        interval_secs: f64,
        source_rate: u32,
        source_channels: usize,
        source_format: SampleFormat,
    ) -> Self {
        let keep_seconds: usize = 15;
        let max_pcm_samples = 16000 * keep_seconds;
        Self {
            interval_secs,
            source_rate,
            _source_channels: source_channels,
            source_format,
            pcm_buf: Arc::new(Mutex::new(Vec::with_capacity(max_pcm_samples))),
            max_pcm_samples,
            // Start far enough in the past so the first detection fires after
            // a few seconds of audio have been collected rather than immediately.
            last_detect: Instant::now(),
            result: Arc::new(Mutex::new(None)),
            detecting: Arc::new(Mutex::new(false)),
            ever_attempted: Arc::new(Mutex::new(false)),
        }
    }

    // ------------------------------------------------------------------
    // Feed audio
    // ------------------------------------------------------------------

    /// Feed a chunk of multi-channel audio (same format as [`crate::recorder::AudioRecorder::write_audio`]).
    /// The audio is down-mixed to mono, resampled to 16 kHz, converted to i16
    /// and appended to the internal ring buffer.
    pub fn feed_audio(&mut self, audio_data: &[Vec<i32>]) {
        if audio_data.is_empty() || audio_data[0].is_empty() {
            return;
        }

        let frame_count = audio_data[0].len();

        // --- Down-mix to mono (average all channels) and convert to f64 ---
        let scale = match self.source_format {
            SampleFormat::S16 => i16::MAX as f64,
            SampleFormat::S32 => i32::MAX as f64,
        };

        let mut mono: Vec<f64> = Vec::with_capacity(frame_count);
        for i in 0..frame_count {
            let mut sum: f64 = 0.0;
            let mut count = 0u32;
            for ch in audio_data.iter() {
                if i < ch.len() {
                    sum += ch[i] as f64;
                    count += 1;
                }
            }
            if count > 0 {
                mono.push(sum / count as f64 / scale);
            }
        }

        // --- Resample from source_rate to 16 kHz (simple linear interpolation) ---
        let ratio = 16000.0 / self.source_rate as f64;
        let out_len = (mono.len() as f64 * ratio).ceil() as usize;
        let mut resampled: Vec<i16> = Vec::with_capacity(out_len);

        for i in 0..out_len {
            let src_idx = i as f64 / ratio;
            let idx0 = src_idx.floor() as usize;
            let frac = src_idx - idx0 as f64;
            let s0 = mono.get(idx0).copied().unwrap_or(0.0);
            let s1 = mono.get(idx0 + 1).copied().unwrap_or(s0);
            let val = s0 + (s1 - s0) * frac;
            // Clamp to i16 range
            let clamped = (val * i16::MAX as f64).round().max(i16::MIN as f64).min(i16::MAX as f64);
            resampled.push(clamped as i16);
        }

        // --- Append to ring buffer, trimming old data ---
        if let Ok(mut buf) = self.pcm_buf.lock() {
            buf.extend_from_slice(&resampled);
            let excess = buf.len().saturating_sub(self.max_pcm_samples);
            if excess > 0 {
                buf.drain(..excess);
            }
        }
    }

    // ------------------------------------------------------------------
    // Tick — call from the main loop
    // ------------------------------------------------------------------

    /// Check whether it is time to run a detection and, if so, spawn a
    /// background thread.  Call this from the main loop whenever recording
    /// is active.
    pub fn tick(&mut self) {
        let elapsed = self.last_detect.elapsed().as_secs_f64();
        if elapsed < self.interval_secs {
            return;
        }

        // Don't stack detections
        if *self.detecting.lock().unwrap() {
            return;
        }

        // Need at least 3 seconds of audio
        let buf_len = self.pcm_buf.lock().unwrap().len();
        if buf_len < 16000 * 3 {
            return;
        }

        self.last_detect = Instant::now();
        *self.detecting.lock().unwrap() = true;
        *self.ever_attempted.lock().unwrap() = true;

        // Take a snapshot of the PCM buffer
        let samples: Vec<i16> = self.pcm_buf.lock().unwrap().clone();
        let result_ref = Arc::clone(&self.result);
        let detecting_ref = Arc::clone(&self.detecting);

        thread::spawn(move || {
            let shazam = Shazam::new();
            match shazam.recognize_from_pcm(&samples) {
                Ok(res) => {
                    *result_ref.lock().unwrap() = Some(res);
                }
                Err(e) => {
                    eprintln!("\rSong detection error: {}", e);
                    // Keep previous result on error
                }
            }
            *detecting_ref.lock().unwrap() = false;
        });
    }

    // ------------------------------------------------------------------
    // Query the latest result
    // ------------------------------------------------------------------

    /// Get the latest recognition result (if any).
    pub fn last_result(&self) -> Option<RecognizeResult> {
        self.result.lock().unwrap().clone()
    }

    /// Returns true if a detection request is currently in progress.
    pub fn is_detecting(&self) -> bool {
        *self.detecting.lock().unwrap()
    }

    /// Format a status line suitable for the VU meter display.
    pub fn status_line(&self) -> Option<String> {
        if self.is_detecting() {
            // Show "detecting..." while a request is in flight
            if let Some(prev) = self.last_result() {
                if prev.is_recognized() {
                    return Some(format!("♫ {} (detecting...)", prev));
                }
            }
            return Some("♫ Detecting song...".to_string());
        }

        if let Some(result) = self.last_result() {
            if result.is_recognized() {
                let mut line = format!("♫ {}", result);
                if let Some(ref album) = result.album {
                    line.push_str(&format!(" [{}]", album));
                }
                Some(line)
            } else if *self.ever_attempted.lock().unwrap() {
                Some("♫ (not recognized)".to_string())
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Reset the detection state (e.g. when recording stops).
    pub fn reset(&mut self) {
        if let Ok(mut buf) = self.pcm_buf.lock() {
            buf.clear();
        }
        *self.result.lock().unwrap() = None;
        *self.ever_attempted.lock().unwrap() = false;
    }

    /// Trigger an immediate song detection (e.g., when a song boundary is detected).
    /// Resets the interval timer so the next automatic detection is interval_secs from now.
    pub fn trigger_immediate(&mut self) {
        // Don't stack detections
        if *self.detecting.lock().unwrap() {
            return;
        }

        // Need at least 3 seconds of audio
        let buf_len = self.pcm_buf.lock().unwrap().len();
        if buf_len < 16000 * 3 {
            return;
        }

        // Reset timer so next automatic detection is interval_secs from now
        self.last_detect = Instant::now();
        *self.detecting.lock().unwrap() = true;
        *self.ever_attempted.lock().unwrap() = true;

        // Take a snapshot of the PCM buffer
        let samples: Vec<i16> = self.pcm_buf.lock().unwrap().clone();
        let result_ref = Arc::clone(&self.result);
        let detecting_ref = Arc::clone(&self.detecting);

        thread::spawn(move || {
            let shazam = Shazam::new();
            match shazam.recognize_from_pcm(&samples) {
                Ok(res) => {
                    *result_ref.lock().unwrap() = Some(res);
                }
                Err(e) => {
                    eprintln!("\rSong detection error: {}", e);
                    // Keep previous result on error
                }
            }
            *detecting_ref.lock().unwrap() = false;
        });
    }
}
