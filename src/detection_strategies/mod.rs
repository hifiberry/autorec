//! Alternative pause detection strategies for song boundary detection.
//!
//! This module contains different approaches to detecting pauses between songs:
//! - Absolute threshold (original)
//! - Relative drop detection
//! - Energy ratio detection
//! - Spectral change detection

pub mod absolute_threshold;
pub mod relative_drop;
pub mod energy_ratio;
pub mod transition;
pub mod guided;

use crate::SampleFormat;

#[derive(Debug, Clone, Copy)]
pub enum PauseEvent {
    /// A pause has been detected (song boundary)
    SongBoundary,
}

#[derive(Debug, Clone)]
pub struct DebugInfo {
    pub current_metric: f32,
    pub threshold: f32,
    pub in_pause: bool,
    pub song_count: u32,
    pub strategy_specific: String,
}

/// Common trait for all pause detection strategies
pub trait PauseDetectionStrategy {
    /// Feed audio data and get pause detection events
    fn feed_audio(&mut self, audio: &[Vec<i32>], format: SampleFormat) -> Option<PauseEvent>;
    
    /// Get the current song number
    fn song_number(&self) -> u32;
    
    /// Get status line for display
    fn status_line(&self) -> Option<String>;
    
    /// Reset the detector
    fn reset(&mut self);
    
    /// Get debug information
    fn get_debug_info(&self) -> DebugInfo;
    
    /// Get the strategy name
    fn name(&self) -> &str;
}
