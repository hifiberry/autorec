//! Adaptive pause detector for identifying song boundaries during recording.
//!
//! The detector has two phases:
//! 1. **Training**: Learns the noise floor from the groove-in period (ignoring initial click)
//! 2. **Active**: Detects pauses between songs and adapts thresholds based on detection patterns

use crate::SampleFormat;
use std::time::{Duration, Instant};

const TRAINING_SKIP_MS: u32 = 500;          // Skip first 500ms (click)
const MUSIC_DETECT_DELTA_DB: f32 = 10.0;    // Music is 10dB+ above noise floor
const MUSIC_DETECT_DURATION_MS: u32 = 200;  // Music must be present for 200ms
const MIN_SONG_LENGTH_SECS: u32 = 120;      // If avg < 2min, we're too sensitive
const PAUSE_TIMEOUT_SECS: u32 = 360;        // 6 minutes without pause = reduce sensitivity

#[derive(Debug, Clone)]
pub struct DebugInfo {
    pub current_rms_db: f32,
    pub threshold_db: f32,
    pub noise_floor_db: f32,
    pub pause_duration_ms: u32,
    pub in_pause: bool,
    pub song_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DetectorState {
    /// Learning the noise floor from groove-in
    Training,
    /// Actively detecting pauses
    Active,
}

#[derive(Debug, Clone, Copy)]
pub enum PauseEvent {
    /// A pause has been detected (song boundary)
    SongBoundary,
}

pub struct AdaptivePauseDetector {
    state: DetectorState,
    
    // Training phase
    training_start: Instant,
    training_rms_samples: Vec<f32>,
    music_detect_start: Option<Instant>,
    noise_floor_db: f32,
    
    // Adaptive pause detection parameters
    pause_threshold_db: f32,      // RMS must be below this
    pause_duration_ms: u32,       // For this long
    threshold_override: Option<f32>,
    pause_duration_override: Option<u32>,
    
    // Current pause state
    in_pause: bool,
    pause_start: Option<Instant>,
    current_rms_db: f32,
    
    // Song tracking
    song_count: u32,
    current_song_start: Instant,
    song_durations: Vec<Duration>,
    last_pause_time: Instant,
    
    // Audio parameters
    _sample_rate: u32,
}

impl AdaptivePauseDetector {
    /// Create a new adaptive pause detector
    pub fn new(sample_rate: u32) -> Self {
        let now = Instant::now();
        Self {
            state: DetectorState::Training,
            training_start: now,
            training_rms_samples: Vec::new(),
            music_detect_start: None,
            noise_floor_db: -80.0,  // Will be learned
            
            pause_threshold_db: -50.0,  // Initial default
            pause_duration_ms: 200,     // Initial default
            threshold_override: None,
            pause_duration_override: None,
            
            in_pause: false,
            pause_start: None,
            current_rms_db: -80.0,
            
            song_count: 1,  // Start at song 1
            current_song_start: now,
            song_durations: Vec::new(),
            last_pause_time: now,
            
            _sample_rate: sample_rate,
        }
    }
    
    /// Feed audio data and get pause detection events
    pub fn feed_audio(
        &mut self,
        audio: &[Vec<i32>],
        format: SampleFormat,
    ) -> Option<PauseEvent> {
        if audio.is_empty() || audio[0].is_empty() {
            return None;
        }
        
        // Calculate RMS of this chunk
        let rms_db = self.calculate_rms_db(audio, format);
        self.current_rms_db = rms_db;
        
        match self.state {
            DetectorState::Training => self.process_training(rms_db),
            DetectorState::Active => self.process_active(rms_db),
        }
    }
    
    /// Get the current song number
    pub fn song_number(&self) -> u32 {
        self.song_count
    }
    
    /// Get status line for display
    pub fn status_line(&self) -> Option<String> {
        if self.state == DetectorState::Training {
            Some("⏳ Learning noise floor...".to_string())
        } else {
            Some(format!("🎵 Song #{}", self.song_count))
        }
    }
    
    /// Reset the detector (e.g., when recording stops)
    pub fn reset(&mut self) {
        let now = Instant::now();
        self.state = DetectorState::Training;
        self.training_start = now;
        self.training_rms_samples.clear();
        self.music_detect_start = None;
        self.in_pause = false;
        self.pause_start = None;
        self.song_count = 1;
        self.current_song_start = now;
        self.song_durations.clear();
        self.last_pause_time = now;
        self.pause_threshold_db = -50.0;
        self.pause_duration_ms = 200;
    }
    
    /// Override the pause threshold (for tuning/testing)
    pub fn set_threshold_override(&mut self, threshold_db: f32) {
        self.threshold_override = Some(threshold_db);
        self.pause_threshold_db = threshold_db;
    }
    
    /// Override the pause duration requirement (for tuning/testing)
    pub fn set_pause_duration_override(&mut self, duration_ms: u32) {
        self.pause_duration_override = Some(duration_ms);
        self.pause_duration_ms = duration_ms;
    }
    
    /// Get debug information about the current detection state
    pub fn get_debug_info(&self) -> DebugInfo {
        DebugInfo {
            current_rms_db: self.current_rms_db,
            threshold_db: self.pause_threshold_db,
            noise_floor_db: self.noise_floor_db,
            pause_duration_ms: self.pause_duration_ms,
            in_pause: self.in_pause,
            song_count: self.song_count,
        }
    }
    
    // ========== Private methods ==========
    
    /// Calculate RMS in dB for the audio chunk (mix all channels to mono)
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
        
        // Mix to mono and calculate RMS
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
        
        // Convert to dB
        if rms > 0.0 {
            20.0 * rms.log10()
        } else {
            -80.0
        }
    }
    
    /// Process audio during training phase
    fn process_training(&mut self, rms_db: f32) -> Option<PauseEvent> {
        let elapsed = self.training_start.elapsed();
        
        // Skip the first 500ms (click)
        if elapsed.as_millis() < TRAINING_SKIP_MS as u128 {
            return None;
        }
        
        // Collect RMS samples for noise floor estimation
        self.training_rms_samples.push(rms_db);
        
        // Calculate current noise floor estimate (median of samples so far)
        let current_noise_floor = self.estimate_noise_floor();
        
        // Check if music has started (RMS > noise_floor + 10dB)
        if rms_db > current_noise_floor + MUSIC_DETECT_DELTA_DB {
            // Start or continue music detection timer
            if self.music_detect_start.is_none() {
                self.music_detect_start = Some(Instant::now());
            } else if let Some(detect_start) = self.music_detect_start {
                // Check if music has been detected for long enough
                if detect_start.elapsed().as_millis() >= MUSIC_DETECT_DURATION_MS as u128 {
                    // Music confirmed! Transition to active mode
                    self.noise_floor_db = current_noise_floor;
                    self.pause_threshold_db = self.noise_floor_db.max(-50.0);
                    self.state = DetectorState::Active;
                    self.current_song_start = Instant::now();
                    self.last_pause_time = Instant::now();
                    eprintln!("Pause detector: Training complete. Noise floor: {:.1} dB, Threshold: {:.1} dB", 
                             self.noise_floor_db, self.pause_threshold_db);
                }
            }
        } else {
            // Reset music detection if RMS drops
            self.music_detect_start = None;
        }
        
        None
    }
    
    /// Estimate noise floor from training samples (use median for robustness)
    fn estimate_noise_floor(&self) -> f32 {
        if self.training_rms_samples.is_empty() {
            return -80.0;
        }
        
        let mut sorted = self.training_rms_samples.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        let mid = sorted.len() / 2;
        sorted[mid]
    }
    
    /// Process audio during active detection phase
    fn process_active(&mut self, rms_db: f32) -> Option<PauseEvent> {
        // Check if we're in a pause (RMS below threshold)
        let is_below_threshold = rms_db < self.pause_threshold_db;
        
        if is_below_threshold {
            // Start or continue pause
            if !self.in_pause {
                self.in_pause = true;
                self.pause_start = Some(Instant::now());
            }
        } else {
            // Above threshold - check if we were in a pause
            if self.in_pause {
                if let Some(start) = self.pause_start {
                    let pause_duration_ms = start.elapsed().as_millis() as u32;
                    
                    // Was the pause long enough?
                    if pause_duration_ms >= self.pause_duration_ms {
                        // Song boundary detected!
                        let song_duration = self.current_song_start.elapsed();
                        self.song_durations.push(song_duration);
                        self.song_count += 1;
                        self.current_song_start = Instant::now();
                        self.last_pause_time = Instant::now();
                        
                        // Apply adaptive logic
                        self.adapt_parameters();
                        
                        // Reset pause state
                        self.in_pause = false;
                        self.pause_start = None;
                        
                        return Some(PauseEvent::SongBoundary);
                    }
                }
                
                // Pause was too short, ignore it
                self.in_pause = false;
                self.pause_start = None;
            }
        }
        
        // Check for timeout (no pause detected for 6 minutes)
        if self.last_pause_time.elapsed().as_secs() >= PAUSE_TIMEOUT_SECS as u64 {
            self.increase_sensitivity();
            self.last_pause_time = Instant::now();  // Reset timeout
        }
        
        None
    }
    
    /// Adapt detection parameters based on song length patterns
    fn adapt_parameters(&mut self) {
        if self.song_durations.len() < 3 {
            return;  // Need at least 3 songs to adapt
        }
        
        // Calculate average song length
        let total: Duration = self.song_durations.iter().sum();
        let avg_secs = total.as_secs() / self.song_durations.len() as u64;
        
        eprintln!("Pause detector: Avg song length: {}s (from {} songs)", avg_secs, self.song_durations.len());
        
        // If average song length is too short, we're detecting too many pauses
        if avg_secs < MIN_SONG_LENGTH_SECS as u64 {
            // Increase required pause duration (make detection less sensitive)
            // But don't override if user has set manual override
            if self.pause_duration_override.is_none() {
                let old_duration = self.pause_duration_ms;
                self.pause_duration_ms = (self.pause_duration_ms + 100).min(1500);
                eprintln!("Pause detector: Songs too short, increasing pause duration: {}ms -> {}ms", 
                         old_duration, self.pause_duration_ms);
            }
        }
    }
    
    /// Increase sensitivity when no pauses detected for a long time
    fn increase_sensitivity(&mut self) {
        // Don't override if user has set manual threshold
        if self.threshold_override.is_some() {
            return;
        }
        
        // Decrease the RMS threshold (make it easier to detect pauses)
        let old_threshold = self.pause_threshold_db;
        self.pause_threshold_db = (self.pause_threshold_db + 2.0).min(-30.0);
        eprintln!("Pause detector: No pause for 6min, increasing sensitivity: {:.1}dB -> {:.1}dB", 
                 old_threshold, self.pause_threshold_db);
    }
}
