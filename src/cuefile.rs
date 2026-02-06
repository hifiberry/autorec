//! CUE sheet generation and management for vinyl recordings.
//!
//! This module handles creating CUE files with proper timestamps,
//! detecting existing CUE files, and managing the .cue vs .guess.cue
//! naming convention based on MusicBrainz match status.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Represents a detected valley (potential song boundary)
#[derive(Debug, Clone)]
pub struct Valley {
    pub position_seconds: f64,
    pub depth_db: f32,
    pub prominence_db: f32,
    pub left_level_db: f32,
    pub right_level_db: f32,
    pub width_seconds: f64,
    pub score: f64,
}

/// Generate CUE file content from track boundaries.
///
/// # Arguments
/// * `wav_file` - Path to the WAV file
/// * `artist` - Artist name for the CUE sheet
/// * `title` - Album/release title for the CUE sheet
/// * `track_names` - Names for each track (optional)
/// * `groove_in` - Start time of first track in seconds
/// * `boundaries` - Valley positions representing track boundaries
///
/// # Returns
/// Complete CUE file content as a string
pub fn generate_cue_file(
    wav_file: &str,
    artist: &str,
    title: &str,
    track_names: &[String],
    groove_in: f64,
    boundaries: &[Valley],
) -> String {
    let wav_filename = Path::new(wav_file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.wav");
    
    let mut cue = String::new();
    cue.push_str(&format!("REM GENERATOR \"HiFiBerry AutoRec boundary_finder\"\n"));
    cue.push_str(&format!("PERFORMER \"{}\"\n", artist));
    cue.push_str(&format!("TITLE \"{}\"\n", title));
    cue.push_str(&format!("FILE \"{}\" WAVE\n", wav_filename));
    
    let mut track_positions = vec![groove_in];
    for b in boundaries {
        track_positions.push(b.position_seconds);
    }
    
    for (i, &pos) in track_positions.iter().enumerate() {
        let track_num = i + 1;
        let default_name = format!("Track {}", track_num);
        let track_name = track_names.get(i)
            .map(|n| n.as_str())
            .unwrap_or(&default_name);
        
        // Remove track number prefix if present (e.g., "#1 Song" -> "Song")
        let prefix = format!("#{} ", track_num);
        let clean_name = if let Some(stripped) = track_name.strip_prefix(&prefix) {
            stripped
        } else {
            track_name
        };
        
        cue.push_str(&format!("  TRACK {:02} AUDIO\n", track_num));
        cue.push_str(&format!("    TITLE \"{}\"\n", clean_name));
        cue.push_str(&format!("    PERFORMER \"{}\"\n", artist));
        
        // Convert position to MM:SS:FF (frames, 75 per second)
        let minutes = (pos / 60.0) as u32;
        let seconds = (pos % 60.0) as u32;
        let frames = ((pos % 1.0) * 75.0) as u32;
        cue.push_str(&format!("    INDEX 01 {:02}:{:02}:{:02}\n", minutes, seconds, frames));
    }
    
    cue
}

/// Write CUE file content to disk.
///
/// # Arguments
/// * `wav_file` - Path to the WAV file (used to derive CUE file path)
/// * `cue_content` - Complete CUE file content
/// * `has_mb_match` - Whether this recording was matched with MusicBrainz
///
/// # Returns
/// Path to the created CUE file, or an error
///
/// # Naming Convention
/// * If `has_mb_match` is true: Creates `.cue` file (verified track data)
/// * If `has_mb_match` is false: Creates `.guess.cue` file (autonomous detection)
pub fn write_cue_file(wav_file: &str, cue_content: &str, has_mb_match: bool) -> Result<PathBuf, std::io::Error> {
    let base_path = Path::new(wav_file).with_extension("");
    let cue_path = if has_mb_match {
        base_path.with_extension("cue")
    } else {
        // No MusicBrainz match - use .guess.cue suffix
        PathBuf::from(format!("{}.guess.cue", base_path.display()))
    };
    let mut file = File::create(&cue_path)?;
    file.write_all(cue_content.as_bytes())?;
    Ok(cue_path)
}

/// Check if a CUE file exists for the given WAV file.
///
/// # Arguments
/// * `wav_file` - Path to the WAV file
///
/// # Returns
/// True if either `.cue` or `.guess.cue` file exists
pub fn has_cue_file(wav_file: &str) -> bool {
    let cue_path = Path::new(wav_file).with_extension("cue");
    let base_path = Path::new(wav_file).with_extension("");
    let guess_cue_path = PathBuf::from(format!("{}.guess.cue", base_path.display()));
    cue_path.exists() || guess_cue_path.exists()
}

/// Generate detection info text file content.
///
/// # Arguments
/// * `wav_file` - Path to the WAV file
/// * `groove_in` - Detected groove-in time in seconds
/// * `groove_out` - Detected groove-out time in seconds
/// * `boundaries` - Detected boundaries
/// * `track_names` - Track names (if available)
/// * `expected_tracks` - Expected track data from MusicBrainz (if available)
/// * `mb_info` - MusicBrainz release information string
///
/// # Returns
/// Text content for the info file
pub fn generate_info_file(
    wav_file: &str,
    groove_in: f64,
    groove_out: f64,
    boundaries: &[Valley],
    track_names: &[String],
    expected_tracks: Option<&[(f64, f64)]>, // (expected_start, expected_length)
    mb_info: Option<&str>,
) -> String {
    let mut info = String::new();
    
    info.push_str(&format!("Vinyl Recording Analysis\n"));
    info.push_str(&format!("========================\n\n"));
    info.push_str(&format!("File: {}\n\n", Path::new(wav_file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(wav_file)));
    
    // Groove timing
    info.push_str(&format!("Groove Timing:\n"));
    info.push_str(&format!("--------------\n"));
    info.push_str(&format!("Lead-in (groove-in):  {:.2}s\n", groove_in));
    info.push_str(&format!("Lead-out (groove-out): {:.2}s\n\n", groove_out));
    
    // MusicBrainz info
    if let Some(mb) = mb_info {
        info.push_str(&format!("MusicBrainz Match:\n"));
        info.push_str(&format!("------------------\n"));
        info.push_str(&format!("{}\n\n", mb));
    }
    
    // Detection method
    let detection_method = if expected_tracks.is_some() {
        "Guided (MusicBrainz-based)"
    } else {
        "Autonomous (valley-based)"
    };
    info.push_str(&format!("Detection Method: {}\n\n", detection_method));
    
    // Track boundaries and adjustments
    if !boundaries.is_empty() {
        info.push_str(&format!("Track Boundaries:\n"));
        info.push_str(&format!("-----------------\n"));
        
        let mut current_pos = groove_in;
        for (i, boundary) in boundaries.iter().enumerate() {
            let track_num = i + 1;
            let track_name = track_names.get(i)
                .map(|n| n.as_str())
                .unwrap_or("Unknown");
            
            info.push_str(&format!("Track {}: {}\n", track_num, track_name));
            info.push_str(&format!("  Start: {:.2}s\n", current_pos));
            info.push_str(&format!("  End:   {:.2}s\n", boundary.position_seconds));
            info.push_str(&format!("  Duration: {:.2}s\n", boundary.position_seconds - current_pos));
            
            // Show adjustment if we have expected data
            if let Some(expected) = expected_tracks {
                if i < expected.len() {
                    let (expected_start, expected_length) = expected[i];
                    let actual_start = current_pos - groove_in;
                    let actual_length = boundary.position_seconds - current_pos;
                    let start_diff = actual_start - expected_start;
                    let length_diff = actual_length - expected_length;
                    
                    info.push_str(&format!("  Expected start: {:.2}s (offset: {:+.2}s)\n", 
                                          expected_start, start_diff));
                    info.push_str(&format!("  Expected length: {:.2}s (diff: {:+.2}s)\n",
                                          expected_length, length_diff));
                }
            }
            info.push_str("\n");
            
            current_pos = boundary.position_seconds;
        }
        
        // Last track
        let last_track = boundaries.len() + 1;
        let last_name = track_names.get(boundaries.len())
            .map(|n| n.as_str())
            .unwrap_or("Unknown");
        info.push_str(&format!("Track {}: {}\n", last_track, last_name));
        info.push_str(&format!("  Start: {:.2}s\n", current_pos));
        info.push_str(&format!("  End:   {:.2}s\n", groove_out));
        info.push_str(&format!("  Duration: {:.2}s\n", groove_out - current_pos));
        
        if let Some(expected) = expected_tracks {
            if boundaries.len() < expected.len() {
                let (expected_start, expected_length) = expected[boundaries.len()];
                let actual_start = current_pos - groove_in;
                let actual_length = groove_out - current_pos;
                let start_diff = actual_start - expected_start;
                let length_diff = actual_length - expected_length;
                
                info.push_str(&format!("  Expected start: {:.2}s (offset: {:+.2}s)\n",
                                      expected_start, start_diff));
                info.push_str(&format!("  Expected length: {:.2}s (diff: {:+.2}s)\n",
                                      expected_length, length_diff));
            }
        }
    }
    
    info
}

/// Write info text file.
///
/// # Arguments
/// * `wav_file` - Path to the WAV file (used to derive info file path)
/// * `info_content` - Complete info file content
/// * `has_mb_match` - Whether this recording was matched with MusicBrainz
///
/// # Returns
/// Path to the created info file, or an error
pub fn write_info_file(wav_file: &str, info_content: &str, has_mb_match: bool) -> Result<PathBuf, std::io::Error> {
    let base_path = Path::new(wav_file).with_extension("");
    let info_path = if has_mb_match {
        PathBuf::from(format!("{}.cue.txt", base_path.display()))
    } else {
        PathBuf::from(format!("{}.guess.cue.txt", base_path.display()))
    };
    let mut file = File::create(&info_path)?;
    file.write_all(info_content.as_bytes())?;
    Ok(info_path)
}
