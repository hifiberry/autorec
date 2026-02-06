//! Relative drop detection - detects when RMS drops significantly relative to recent average.
//! This adapts to the overall volume level of the recording.

use super::{DebugInfo, PauseDetectionStrategy, PauseEvent};
use crate::SampleFormat;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct RelativeDropDetector {
    sample_rate: u32,
    drop_threshold_db: f32,  // How much below average is considered a pause
    pause_duration_ms: u32,
    window_seconds: f32,      // How many seconds to average over
    
    current_rms_db: f32,
    rms_history: VecDeque<f32>,
    max_history_size: usize,
    
    in_pause: bool,
    pause_start: Option<Instant>,
    song_count: u32,
    current_song_start: Instant,
}

impl RelativeDropDetector {
    pub fn new(sample_rate: u32, drop_threshold_db: f32, pause_duration_ms: u32, window_seconds: f32) -> Self {
        let chunk_duration_sec = 0.2; // Assuming 200ms chunks
        let max_history_size = (window_seconds / chunk_duration_sec) as usize;
        
        Self {
            sample_rate,
            drop_threshold_db,
            pause_duration_ms,
            window_seconds,
            current_rms_db: -80.0,
            rms_history: VecDeque::with_capacity(max_history_size),
            max_history_size,
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
    
    fn get_average_rms(&self) -> f32 {
        if self.rms_history.is_empty() {
            return -80.0;
        }
        
        let sum: f32 = self.rms_history.iter().sum();
        sum / self.rms_history.len() as f32
    }
}

impl PauseDetectionStrategy for RelativeDropDetector {
    fn feed_audio(&mut self, audio: &[Vec<i32>], format: SampleFormat) -> Option<PauseEvent> {
        if audio.is_empty() || audio[0].is_empty() {
            return None;
        }
        
        self.current_rms_db = self.calculate_rms_db(audio, format);
        
        // Add to history
        self.rms_history.push_back(self.current_rms_db);
        if self.rms_history.len() > self.max_history_size {
            self.rms_history.pop_front();
        }
        
        // Need enough history to make decisions
        if self.rms_history.len() < 5 {
            return None;
        }
        
        let avg_rms = self.get_average_rms();
        let drop_from_average = avg_rms - self.current_rms_db;
        let is_below_threshold = drop_from_average > self.drop_threshold_db;
        
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
                        
                        // Clear history on song boundary to adapt to new song level
                        self.rms_history.clear();
                        
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
        self.rms_history.clear();
        self.in_pause = false;
        self.pause_start = None;
        self.song_count = 1;
        self.current_song_start = Instant::now();
    }
    
    fn get_debug_info(&self) -> DebugInfo {
        let avg_rms = self.get_average_rms();
        let drop = avg_rms - self.current_rms_db;
        
        DebugInfo {
            current_metric: drop,
            threshold: self.drop_threshold_db,
            in_pause: self.in_pause,
            song_count: self.song_count,
            strategy_specific: format!("RMS: {:.1} dB, Avg: {:.1} dB, Drop: {:.1} dB", 
                                      self.current_rms_db, avg_rms, drop),
        }
    }
    
    fn name(&self) -> &str {
        "Relative Drop"
    }
}
