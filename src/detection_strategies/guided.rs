//! Guided detection - uses expected track boundaries from MusicBrainz to guide pause detection.
//! Looks for the quietest point within a search window around expected boundaries.

use super::{DebugInfo, PauseDetectionStrategy, PauseEvent};
use crate::musicbrainz::ExpectedTrack;
use crate::SampleFormat;
use std::collections::VecDeque;
use std::time::Instant;

pub struct GuidedDetector {
    sample_rate: u32,
    search_window_seconds: f64,  // How far to search before/after expected boundary
    
    expected_tracks: Vec<ExpectedTrack>,
    current_position_seconds: f64,
    current_rms_db: f32,
    
    // Track the minimum RMS in the current search window
    in_search_window: bool,
    search_window_start: f64,
    search_window_end: f64,
    min_rms_in_window: f32,
    min_rms_position: f64,
    next_boundary_index: usize,
    
    rms_history: VecDeque<(f64, f32)>,  // (timestamp, rms_db)
    max_history_size: usize,
    
    song_count: u32,
    detected_boundaries: Vec<f64>,
}

impl GuidedDetector {
    pub fn new(sample_rate: u32, expected_tracks: Vec<ExpectedTrack>, search_window_seconds: f64) -> Self {
        let max_history_size = 500;  // Keep last ~100 seconds at 200ms chunks
        
        Self {
            sample_rate,
            search_window_seconds,
            expected_tracks,
            current_position_seconds: 0.0,
            current_rms_db: -80.0,
            in_search_window: false,
            search_window_start: 0.0,
            search_window_end: 0.0,
            min_rms_in_window: 0.0,
            min_rms_position: 0.0,
            next_boundary_index: 1,  // Start looking for boundary after track 1
            rms_history: VecDeque::with_capacity(max_history_size),
            max_history_size,
            song_count: 1,
            detected_boundaries: Vec::new(),
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
    
    fn get_expected_boundary(&self, index: usize) -> Option<f64> {
        if index < self.expected_tracks.len() {
            Some(self.expected_tracks[index].expected_start)
        } else {
            None
        }
    }
}

impl PauseDetectionStrategy for GuidedDetector {
    fn feed_audio(&mut self, audio: &[Vec<i32>], format: SampleFormat) -> Option<PauseEvent> {
        if audio.is_empty() || audio[0].is_empty() {
            return None;
        }
        
        let num_samples = audio[0].len();
        let chunk_duration = num_samples as f64 / self.sample_rate as f64;
        
        self.current_rms_db = self.calculate_rms_db(audio, format);
        
        // Add to history
        self.rms_history.push_back((self.current_position_seconds, self.current_rms_db));
        if self.rms_history.len() > self.max_history_size {
            self.rms_history.pop_front();
        }
        
        // Check if we need to start a search window
        if !self.in_search_window {
            if let Some(expected_boundary) = self.get_expected_boundary(self.next_boundary_index) {
                let time_until_boundary = expected_boundary - self.current_position_seconds;
                
                // Start search window when we're within range
                if time_until_boundary <= self.search_window_seconds {
                    self.in_search_window = true;
                    self.search_window_start = (expected_boundary - self.search_window_seconds).max(0.0);
                    self.search_window_end = expected_boundary + self.search_window_seconds;
                    self.min_rms_in_window = self.current_rms_db;
                    self.min_rms_position = self.current_position_seconds;
                    
                    eprintln!("Searching for boundary #{} around {:.1}s (±{:.1}s)", 
                             self.next_boundary_index, expected_boundary, self.search_window_seconds);
                }
            }
        }
        
        // If in search window, track minimum RMS
        if self.in_search_window {
            if self.current_rms_db < self.min_rms_in_window {
                self.min_rms_in_window = self.current_rms_db;
                self.min_rms_position = self.current_position_seconds;
            }
            
            // Check if we've passed the end of the window
            if self.current_position_seconds > self.search_window_end {
                // Boundary detected at minimum point
                self.song_count += 1;
                self.detected_boundaries.push(self.min_rms_position);
                self.next_boundary_index += 1;
                self.in_search_window = false;
                
                eprintln!("Boundary detected at {:.2}s (RMS: {:.1}dB)", 
                         self.min_rms_position, self.min_rms_in_window);
                
                self.current_position_seconds += chunk_duration;
                return Some(PauseEvent::SongBoundary);
            }
        }
        
        self.current_position_seconds += chunk_duration;
        None
    }
    
    fn song_number(&self) -> u32 {
        self.song_count
    }
    
    fn status_line(&self) -> Option<String> {
        if self.next_boundary_index < self.expected_tracks.len() {
            Some(format!("🎵 Song #{} - {}", 
                        self.song_count, 
                        self.expected_tracks[self.song_count as usize - 1].title))
        } else {
            Some(format!("🎵 Song #{}", self.song_count))
        }
    }
    
    fn reset(&mut self) {
        self.current_position_seconds = 0.0;
        self.rms_history.clear();
        self.in_search_window = false;
        self.next_boundary_index = 1;
        self.song_count = 1;
        self.detected_boundaries.clear();
    }
    
    fn get_debug_info(&self) -> DebugInfo {
        let status = if self.in_search_window {
            format!("Searching window {:.1}s-{:.1}s, min RMS: {:.1}dB @ {:.1}s",
                   self.search_window_start, self.search_window_end,
                   self.min_rms_in_window, self.min_rms_position)
        } else if let Some(expected) = self.get_expected_boundary(self.next_boundary_index) {
            format!("Next boundary expected @ {:.1}s (in {:.1}s)",
                   expected, expected - self.current_position_seconds)
        } else {
            "No more expected boundaries".to_string()
        };
        
        DebugInfo {
            current_metric: self.current_rms_db,
            threshold: self.min_rms_in_window,
            in_pause: self.in_search_window,
            song_count: self.song_count,
            strategy_specific: status,
        }
    }
    
    fn name(&self) -> &str {
        "Guided (MusicBrainz)"
    }
}
