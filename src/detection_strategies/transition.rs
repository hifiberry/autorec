//! Combined transition detector - looks for quiet periods followed by sudden energy increases.
//! This works well for continuous recordings where there's no true silence.

use super::{DebugInfo, PauseDetectionStrategy, PauseEvent};
use crate::SampleFormat;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct TransitionDetector {
    sample_rate: u32,
    quiet_threshold_percentile: f32,  // What percentile of energy counts as "quiet" (e.g. 0.2 = bottom 20%)
    rise_threshold_db: f32,            // How much RMS must jump to indicate new song
    min_quiet_duration_ms: u32,        // How long must be quiet before looking for rise
    window_seconds: f32,               // Window for calculating percentiles
    
    current_rms_db: f32,
    rms_history: VecDeque<f32>,
    max_history_size: usize,
    
    in_quiet_period: bool,
    quiet_start: Option<Instant>,
    quiet_start_rms: f32,
    song_count: u32,
    current_song_start: Instant,
}

impl TransitionDetector {
    pub fn new(
        sample_rate: u32,
        quiet_threshold_percentile: f32,
        rise_threshold_db: f32,
        min_quiet_duration_ms: u32,
        window_seconds: f32,
    ) -> Self {
        let chunk_duration_sec = 0.2;
        let max_history_size = (window_seconds / chunk_duration_sec) as usize;
        
        Self {
            sample_rate,
            quiet_threshold_percentile,
            rise_threshold_db,
            min_quiet_duration_ms,
            window_seconds,
            current_rms_db: -80.0,
            rms_history: VecDeque::with_capacity(max_history_size),
            max_history_size,
            in_quiet_period: false,
            quiet_start: None,
            quiet_start_rms: -80.0,
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
    
    fn get_percentile_threshold(&self) -> f32 {
        if self.rms_history.len() < 10 {
            return -80.0;
        }
        
        let mut sorted: Vec<f32> = self.rms_history.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        let idx = (sorted.len() as f32 * self.quiet_threshold_percentile) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
}

impl PauseDetectionStrategy for TransitionDetector {
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
        
        // Need enough history
        if self.rms_history.len() < 10 {
            return None;
        }
        
        let quiet_threshold = self.get_percentile_threshold();
        let is_quiet = self.current_rms_db < quiet_threshold;
        
        if is_quiet {
            // Start quiet period
            if !self.in_quiet_period {
                self.in_quiet_period = true;
                self.quiet_start = Some(Instant::now());
                self.quiet_start_rms = self.current_rms_db;
            }
        } else {
            // Not quiet anymore
            if self.in_quiet_period {
                if let Some(start) = self.quiet_start {
                    let quiet_duration_ms = start.elapsed().as_millis() as u32;
                    
                    // Was quiet long enough AND did RMS jump significantly?
                    let rms_jump = self.current_rms_db - self.quiet_start_rms;
                    
                    if quiet_duration_ms >= self.min_quiet_duration_ms && rms_jump >= self.rise_threshold_db {
                        // Song boundary detected!
                        self.song_count += 1;
                        self.current_song_start = Instant::now();
                        
                        // Clear history to adapt to new song
                        self.rms_history.clear();
                        
                        self.in_quiet_period = false;
                        self.quiet_start = None;
                        return Some(PauseEvent::SongBoundary);
                    }
                }
                
                self.in_quiet_period = false;
                self.quiet_start = None;
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
        self.in_quiet_period = false;
        self.quiet_start = None;
        self.song_count = 1;
        self.current_song_start = Instant::now();
    }
    
    fn get_debug_info(&self) -> DebugInfo {
        let threshold = self.get_percentile_threshold();
        
        DebugInfo {
            current_metric: self.current_rms_db,
            threshold,
            in_pause: self.in_quiet_period,
            song_count: self.song_count,
            strategy_specific: format!("RMS: {:.1} dB, Quiet thresh ({}%ile): {:.1} dB", 
                                      self.current_rms_db,
                                      (self.quiet_threshold_percentile * 100.0) as u32,
                                      threshold),
        }
    }
    
    fn name(&self) -> &str {
        "Transition (Quiet+Rise)"
    }
}
