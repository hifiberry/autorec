//! Energy ratio detection - compares current energy to recent maximum energy.
//! Detects pauses when energy drops to a small fraction of peak energy.

use super::{DebugInfo, PauseDetectionStrategy, PauseEvent};
use crate::SampleFormat;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct EnergyRatioDetector {
    sample_rate: u32,
    ratio_threshold: f32,     // Pause when current/max < this (e.g., 0.01 = 1%)
    pause_duration_ms: u32,
    window_seconds: f32,       // How many seconds to track max energy over
    
    current_energy: f32,
    energy_history: VecDeque<f32>,
    max_history_size: usize,
    
    in_pause: bool,
    pause_start: Option<Instant>,
    song_count: u32,
    current_song_start: Instant,
}

impl EnergyRatioDetector {
    pub fn new(sample_rate: u32, ratio_threshold: f32, pause_duration_ms: u32, window_seconds: f32) -> Self {
        let chunk_duration_sec = 0.2; // Assuming 200ms chunks
        let max_history_size = (window_seconds / chunk_duration_sec) as usize;
        
        Self {
            sample_rate,
            ratio_threshold,
            pause_duration_ms,
            window_seconds,
            current_energy: 0.0,
            energy_history: VecDeque::with_capacity(max_history_size),
            max_history_size,
            in_pause: false,
            pause_start: None,
            song_count: 1,
            current_song_start: Instant::now(),
        }
    }
    
    fn calculate_energy(&self, audio: &[Vec<i32>], format: SampleFormat) -> f32 {
        let num_channels = audio.len();
        let num_samples = audio[0].len();
        
        if num_samples == 0 {
            return 0.0;
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
        
        (sum_squares / num_samples as f64) as f32
    }
    
    fn get_max_energy(&self) -> f32 {
        self.energy_history.iter().copied().fold(0.0_f32, f32::max)
    }
    
    fn energy_to_db(energy: f32) -> f32 {
        if energy > 0.0 {
            10.0 * energy.log10()
        } else {
            -80.0
        }
    }
}

impl PauseDetectionStrategy for EnergyRatioDetector {
    fn feed_audio(&mut self, audio: &[Vec<i32>], format: SampleFormat) -> Option<PauseEvent> {
        if audio.is_empty() || audio[0].is_empty() {
            return None;
        }
        
        self.current_energy = self.calculate_energy(audio, format);
        
        // Add to history
        self.energy_history.push_back(self.current_energy);
        if self.energy_history.len() > self.max_history_size {
            self.energy_history.pop_front();
        }
        
        // Need enough history to make decisions
        if self.energy_history.len() < 5 {
            return None;
        }
        
        let max_energy = self.get_max_energy();
        let ratio = if max_energy > 0.0 {
            self.current_energy / max_energy
        } else {
            1.0
        };
        
        let is_below_threshold = ratio < self.ratio_threshold;
        
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
                        
                        // Clear history on song boundary
                        self.energy_history.clear();
                        
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
        self.energy_history.clear();
        self.in_pause = false;
        self.pause_start = None;
        self.song_count = 1;
        self.current_song_start = Instant::now();
    }
    
    fn get_debug_info(&self) -> DebugInfo {
        let max_energy = self.get_max_energy();
        let ratio = if max_energy > 0.0 {
            self.current_energy / max_energy
        } else {
            1.0
        };
        
        DebugInfo {
            current_metric: ratio,
            threshold: self.ratio_threshold,
            in_pause: self.in_pause,
            song_count: self.song_count,
            strategy_specific: format!("Current: {:.1} dB, Max: {:.1} dB, Ratio: {:.3}", 
                                      Self::energy_to_db(self.current_energy),
                                      Self::energy_to_db(max_energy),
                                      ratio),
        }
    }
    
    fn name(&self) -> &str {
        "Energy Ratio"
    }
}
