//! Absolute threshold detection - the original simple approach.
//! Detects pauses when RMS drops below an absolute dB threshold.

use super::{DebugInfo, PauseDetectionStrategy, PauseEvent};
use crate::SampleFormat;
use std::time::{Duration, Instant};

pub struct AbsoluteThresholdDetector {
    sample_rate: u32,
    threshold_db: f32,
    pause_duration_ms: u32,
    
    current_rms_db: f32,
    in_pause: bool,
    pause_start: Option<Instant>,
    song_count: u32,
    current_song_start: Instant,
}

impl AbsoluteThresholdDetector {
    pub fn new(sample_rate: u32, threshold_db: f32, pause_duration_ms: u32) -> Self {
        Self {
            sample_rate,
            threshold_db,
            pause_duration_ms,
            current_rms_db: -80.0,
            in_pause: false,
            pause_start: None,
            song_count: 1,
            current_song_start: Instant::now(),
        }
    }
    
    fn calculate_rms_db(&self, audio: &[Vec<i32>], format: SampleFormat) -> f32 {
        let num_channels = audio.len();
        let num_samples = audio[0].len();
        
        if num_samples == 0 {
            return -80.0;
        }
        
        let max_value = match format {
            SampleFormat::S16 => 32768.0_f32,
            SampleFormat::S32 => 2147483648.0_f32,
        };
        
        let mut sum_squares = 0.0_f64;
        for i in 0..num_samples {
            let mut sample_sum = 0.0_f32;
            for channel in audio {
                sample_sum += channel[i] as f32 / max_value;
            }
            let mono_sample = sample_sum / num_channels as f32;
            sum_squares += (mono_sample * mono_sample) as f64;
        }
        
        let rms = (sum_squares / num_samples as f64).sqrt() as f32;
        
        if rms > 0.0 {
            20.0 * rms.log10()
        } else {
            -80.0
        }
    }
}

impl PauseDetectionStrategy for AbsoluteThresholdDetector {
    fn feed_audio(&mut self, audio: &[Vec<i32>], format: SampleFormat) -> Option<PauseEvent> {
        if audio.is_empty() || audio[0].is_empty() {
            return None;
        }
        
        self.current_rms_db = self.calculate_rms_db(audio, format);
        let is_below_threshold = self.current_rms_db < self.threshold_db;
        
        if is_below_threshold {
            if !self.in_pause {
                self.in_pause = true;
                self.pause_start = Some(Instant::now());
            }
        } else {
            if self.in_pause {
                if let Some(start) = self.pause_start {
                    let pause_duration_ms = start.elapsed().as_millis() as u32;
                    
                    if pause_duration_ms >= self.pause_duration_ms {
                        self.song_count += 1;
                        self.current_song_start = Instant::now();
                        self.in_pause = false;
                        self.pause_start = None;
                        return Some(PauseEvent::SongBoundary);
                    }
                }
                
                self.in_pause = false;
                self.pause_start = None;
            }
        }
        
        None
    }
    
    fn song_number(&self) -> u32 {
        self.song_count
    }
    
    fn status_line(&self) -> Option<String> {
        Some(format!("🎵 Song #{}", self.song_count))
    }
    
    fn reset(&mut self) {
        self.in_pause = false;
        self.pause_start = None;
        self.song_count = 1;
        self.current_song_start = Instant::now();
    }
    
    fn get_debug_info(&self) -> DebugInfo {
        DebugInfo {
            current_metric: self.current_rms_db,
            threshold: self.threshold_db,
            in_pause: self.in_pause,
            song_count: self.song_count,
            strategy_specific: format!("RMS: {:.1} dB", self.current_rms_db),
        }
    }
    
    fn name(&self) -> &str {
        "Absolute Threshold"
    }
}
