//! Offline song boundary finder - finds song boundaries in WAV files without external metadata.
//!
//! Three-pass algorithm for vinyl recordings:
//!   Pass 1: Compute RMS in small windows across the entire file
//!   Pass 2: Detect groove-in (start of music) and groove-out (end of music)
//!   Pass 3: Find "valleys" (local minima) that represent song boundaries
//!           within the music region only
//!
//! Vinyl recording characteristics:
//!   - Groove-in: 0.5-5s of quiet groove noise before music starts
//!   - Groove-out: can be minutes of quiet at the end after music stops
//!   - Song boundaries: brief energy dips (not true silence) between tracks
//!   - No absolute silence: groove noise is always present

use autorec::SampleFormat;
use autorec::musicbrainz;
use std::env;
use std::fs::{File, self};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process;

#[derive(Debug)]
struct WavHeader {
    sample_rate: u32,
    num_channels: u16,
    bits_per_sample: u16,
    data_size: u32,
}

fn read_wav_header(file: &mut BufReader<File>) -> Result<WavHeader, String> {
    let mut buf = [0u8; 44];
    file.read_exact(&mut buf).map_err(|e| format!("Failed to read WAV header: {}", e))?;
    
    if &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" || &buf[12..16] != b"fmt " {
        return Err("Not a valid WAV file".to_string());
    }
    
    let num_channels = u16::from_le_bytes([buf[22], buf[23]]);
    let sample_rate = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let bits_per_sample = u16::from_le_bytes([buf[34], buf[35]]);
    
    file.seek(SeekFrom::Start(36)).map_err(|e| format!("Seek error: {}", e))?;
    
    loop {
        let mut chunk_header = [0u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            return Err("Could not find data chunk".to_string());
        }
        
        let chunk_size = u32::from_le_bytes([chunk_header[4], chunk_header[5], chunk_header[6], chunk_header[7]]);
        
        if &chunk_header[0..4] == b"data" {
            return Ok(WavHeader {
                sample_rate,
                num_channels,
                bits_per_sample,
                data_size: chunk_size,
            });
        }
        
        file.seek(SeekFrom::Current(chunk_size as i64)).map_err(|e| format!("Seek error: {}", e))?;
    }
}

fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0) as u32;
    let secs = seconds % 60.0;
    format!("{:02}:{:05.2}", mins, secs)
}

/// Represents a detected valley (potential song boundary)
#[derive(Debug, Clone)]
struct Valley {
    position_seconds: f64,
    depth_db: f32,          // RMS at the minimum point
    prominence_db: f32,     // How much deeper than surrounding average
    left_level_db: f32,     // Average RMS of audio before the valley
    right_level_db: f32,    // Average RMS of audio after the valley
    width_seconds: f64,     // Duration of the energy dip
    score: f64,             // Combined score for ranking
}

/// Compute RMS in dB for a chunk of samples
fn compute_rms_db(audio: &[Vec<i32>], format: SampleFormat) -> f32 {
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

/// Apply a moving average smoothing filter (in linear domain, not dB)
fn smooth_rms(rms_values: &[f32], window_size: usize) -> Vec<f32> {
    let half = window_size / 2;
    let len = rms_values.len();
    let mut smoothed = Vec::with_capacity(len);
    
    let linear: Vec<f64> = rms_values.iter()
        .map(|&db| 10.0_f64.powf(db as f64 / 20.0))
        .collect();
    
    for i in 0..len {
        let start = if i > half { i - half } else { 0 };
        let end = (i + half + 1).min(len);
        let sum: f64 = linear[start..end].iter().sum();
        let avg = sum / (end - start) as f64;
        let db = if avg > 0.0 { 20.0 * avg.log10() } else { -80.0 };
        smoothed.push(db as f32);
    }
    
    smoothed
}

/// Detect the groove-in point (where music starts).
/// Scans from the start for a sustained rise above the midpoint between
/// noise floor and music level.
fn detect_groove_in(
    smoothed: &[f32],
    timestamps: &[f64],
    noise_floor_db: f32,
    music_level_db: f32,
    chunk_duration: f64,
    verbose: bool,
) -> f64 {
    if smoothed.is_empty() {
        return 0.0;
    }
    
    let threshold = (noise_floor_db + music_level_db) / 2.0;
    let sustain_chunks = (2.0 / chunk_duration).max(1.0) as usize;
    
    for i in 0..smoothed.len().saturating_sub(sustain_chunks) {
        if smoothed[i] > threshold {
            let sustained = smoothed[i..i + sustain_chunks].iter().all(|&v| v > threshold);
            if sustained {
                // Walk back to find where the rise started
                let mut start = i;
                while start > 0 && smoothed[start - 1] < smoothed[start] {
                    start -= 1;
                }
                let groove_in = timestamps[start];
                if verbose {
                    println!("  Groove-in detected at {} (threshold: {:.1} dB)",
                             format_timestamp(groove_in), threshold);
                }
                return groove_in;
            }
        }
    }
    
    if verbose {
        println!("  No groove-in detected, using file start");
    }
    0.0
}

/// Detect the groove-out point (where music ends).
/// Scans from the end backwards for the last sustained music region,
/// then finds where the final drop occurs.
fn detect_groove_out(
    smoothed: &[f32],
    timestamps: &[f64],
    noise_floor_db: f32,
    music_level_db: f32,
    file_duration: f64,
    chunk_duration: f64,
    verbose: bool,
) -> f64 {
    if smoothed.is_empty() {
        return file_duration;
    }
    
    let threshold = (noise_floor_db + music_level_db) / 2.0;
    let sustain_chunks = (5.0 / chunk_duration) as usize;
    let len = smoothed.len();
    
    // Scan from end backwards to find the last region with sustained music
    for i in (sustain_chunks..len).rev() {
        let window_start = i.saturating_sub(sustain_chunks);
        let above_count = smoothed[window_start..=i].iter().filter(|&&v| v > threshold).count();
        
        if above_count > sustain_chunks / 2 {
            // Found last music region. Walk forward to find the drop-off.
            for j in i..len {
                if smoothed[j] < threshold {
                    // Check that it stays below for at least 10s
                    let check_end = (j + (10.0 / chunk_duration) as usize).min(len);
                    let stays_below = smoothed[j..check_end].iter().all(|&v| v < threshold);
                    if stays_below {
                        let groove_out = timestamps[j];
                        if verbose {
                            println!("  Groove-out detected at {} (threshold: {:.1} dB, {:.1}s before end)",
                                     format_timestamp(groove_out), threshold, file_duration - groove_out);
                        }
                        return groove_out;
                    }
                }
            }
            break;
        }
    }
    
    if verbose {
        println!("  No groove-out detected, using file end");
    }
    file_duration
}

/// Estimate noise floor: 5th-10th percentile of smoothed RMS
fn estimate_noise_floor(smoothed: &[f32]) -> f32 {
    let mut sorted = smoothed.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p5 = (sorted.len() as f64 * 0.05) as usize;
    let p10 = (sorted.len() as f64 * 0.10) as usize;
    if p10 > p5 {
        sorted[p5..=p10].iter().sum::<f32>() / (p10 - p5 + 1) as f32
    } else {
        sorted[p5.min(sorted.len() - 1)]
    }
}

/// Estimate music level: 60th-80th percentile of smoothed RMS
fn estimate_music_level(smoothed: &[f32]) -> f32 {
    let mut sorted = smoothed.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p60 = (sorted.len() as f64 * 0.60) as usize;
    let p80 = (sorted.len() as f64 * 0.80) as usize;
    if p80 > p60 {
        sorted[p60..=p80].iter().sum::<f32>() / (p80 - p60 + 1) as f32
    } else {
        sorted[p60.min(sorted.len() - 1)]
    }
}

/// Find song boundaries within the music region.
///
/// Algorithm:
///   1. Short smoothing (3s) for precise boundary location
///   2. Long smoothing (30s) for local reference level
///   3. Find local minima in the short-smoothed curve
///   4. Measure prominence = depth below long-smoothed reference
///   5. Measure left/right context levels (15s on each side)
///   6. Require music-level audio on BOTH sides of the valley
///   7. Score by minimum of left-dip and right-dip, scaled by prominence
fn find_song_boundaries(
    rms_values: &[f32],
    timestamps: &[f64],
    smoothed_short: &[f32],
    music_start_idx: usize,
    music_end_idx: usize,
    min_prominence_db: f32,
    min_song_duration_seconds: f64,
    chunk_duration: f64,
    noise_floor_db: f32,
    _music_level_db: f32,
    verbose: bool,
) -> Vec<Valley> {
    let len = music_end_idx.min(rms_values.len());
    if len <= music_start_idx + 10 {
        return Vec::new();
    }
    
    // Long smoothing for reference level (30 seconds)
    let long_window = (30.0 / chunk_duration) as usize;
    let long_smoothed = smooth_rms(rms_values, long_window.max(3));
    
    // Context window: 15 seconds on each side
    let context_chunks = (15.0 / chunk_duration) as usize;
    
    let mut valleys = Vec::new();
    
    // Search radius: 5 seconds for local minimum detection
    let search_radius = (5.0 / chunk_duration) as usize;
    
    for i in (music_start_idx + search_radius)..(len.saturating_sub(search_radius)) {
        let current = smoothed_short[i];
        
        // Check if this is a local minimum
        let range_start = i.saturating_sub(search_radius);
        let range_end = (i + search_radius).min(len - 1);
        let mut is_minimum = true;
        for j in range_start..=range_end {
            if j != i && smoothed_short[j] < current {
                is_minimum = false;
                break;
            }
        }
        if !is_minimum {
            continue;
        }
        
        // Prominence against long-term reference
        let local_ref = long_smoothed[i];
        let prominence = local_ref - current;
        if prominence < min_prominence_db {
            continue;
        }
        
        // Measure left context (audio before the valley)
        let left_start = if i > context_chunks + search_radius {
            i - context_chunks - search_radius
        } else {
            music_start_idx
        };
        let left_end = i.saturating_sub(search_radius / 2);
        let left_level = if left_end > left_start {
            smoothed_short[left_start..left_end].iter().sum::<f32>() / (left_end - left_start) as f32
        } else {
            local_ref
        };
        
        // Measure right context (audio after the valley)
        let right_start = (i + search_radius / 2).min(len - 1);
        let right_end = (i + context_chunks + search_radius).min(len);
        let right_level = if right_end > right_start {
            smoothed_short[right_start..right_end].iter().sum::<f32>() / (right_end - right_start) as f32
        } else {
            local_ref
        };
        
        // Dip from both sides
        let left_dip = left_level - current;
        let right_dip = right_level - current;
        let min_dip = left_dip.min(right_dip);
        
        // Reject if one side is also quiet (within a quiet passage, not between songs)
        if min_dip < min_prominence_db * 0.5 {
            continue;
        }
        
        // Valley width
        let half_prom_threshold = current + prominence / 2.0;
        let mut w_start = i;
        let mut w_end = i;
        while w_start > music_start_idx && smoothed_short[w_start - 1] < half_prom_threshold {
            w_start -= 1;
        }
        while w_end < len - 1 && smoothed_short[w_end + 1] < half_prom_threshold {
            w_end += 1;
        }
        let width = (w_end - w_start) as f64 * chunk_duration;
        
        // Score: emphasise the minimum dip (both sides must have music)
        let score = (min_dip as f64) * (1.0 + prominence as f64 * 0.1) * (1.0 + width.sqrt());
        
        valleys.push(Valley {
            position_seconds: timestamps[i],
            depth_db: current,
            prominence_db: prominence,
            left_level_db: left_level,
            right_level_db: right_level,
            width_seconds: width,
            score,
        });
    }
    
    // Remove valleys too close together (min song duration), keep highest score
    let mut filtered = Vec::new();
    valleys.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    
    for valley in &valleys {
        let too_close = filtered.iter().any(|existing: &Valley| {
            (existing.position_seconds - valley.position_seconds).abs() < min_song_duration_seconds
        });
        if !too_close {
            filtered.push(valley.clone());
        }
    }
    
    filtered.sort_by(|a, b| a.position_seconds.partial_cmp(&b.position_seconds).unwrap());
    
    if verbose && !filtered.is_empty() {
        println!("  Valley candidates before score filtering:");
        for v in &filtered {
            println!("    {} depth={:.1}dB prom={:.1}dB L={:.1}dB R={:.1}dB w={:.1}s score={:.1}",
                     format_timestamp(v.position_seconds),
                     v.depth_db, v.prominence_db,
                     v.left_level_db, v.right_level_db,
                     v.width_seconds, v.score);
        }
    }
    
    // Adaptive score threshold: find the largest gap in the sorted scores.
    // Real song boundaries cluster at high scores, false positives at low scores.
    // The gap between these clusters is the natural threshold.
    if filtered.len() > 1 {
        let mut scores: Vec<f64> = filtered.iter().map(|v| v.score).collect();
        scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        // Find the largest relative gap between consecutive sorted scores
        let mut best_gap_ratio = 0.0_f64;
        let mut best_gap_idx = 0;
        
        for i in 0..scores.len() - 1 {
            let lower = scores[i];
            let upper = scores[i + 1];
            // Use ratio: a gap from 30 to 75 (2.5x) is more significant than 200 to 300 (1.5x)
            if lower > 0.0 {
                let ratio = upper / lower;
                if ratio > best_gap_ratio {
                    best_gap_ratio = ratio;
                    best_gap_idx = i;
                }
            }
        }
        
        // Only apply gap filtering if the gap is significant (> 1.5x difference)
        if best_gap_ratio > 1.5 {
            let threshold = scores[best_gap_idx];
            if verbose {
                println!("  Score gap: {:.1} → {:.1} (ratio {:.1}x), threshold={:.1}",
                         scores[best_gap_idx], scores[best_gap_idx + 1],
                         best_gap_ratio, threshold);
            }
            filtered.retain(|v| v.score > threshold);
        } else if verbose {
            println!("  No significant score gap found (max ratio: {:.1}x)", best_gap_ratio);
        }
        
        // Key insight for vinyl: real song boundaries drop WELL BELOW the noise
        // floor. During a true inter-song gap, the stylus is in an unmodulated
        // groove, producing a signal significantly quieter than the estimated
        // noise floor (which is biased upward by including some musical bleed).
        // Empirically, real boundaries are 7-16 dB below noise floor, while
        // false positives (quiet passages within songs) are at or barely below it.
        // Requiring 5 dB below noise floor cleanly separates them.
        let depth_threshold = noise_floor_db - 5.0;
        let before_depth = filtered.len();
        filtered.retain(|v| v.depth_db <= depth_threshold);
        if verbose {
            println!("  Depth filter: valleys must reach {:.1} dB (noise floor {:.1} dB minus 5 dB margin)",
                     depth_threshold, noise_floor_db);
            if filtered.len() < before_depth {
                println!("    Removed {} valleys that didn't reach deep enough below noise floor",
                         before_depth - filtered.len());
            }
        }
    }
    
    if verbose && !filtered.is_empty() {
        println!("  Final boundaries:");
        for v in &filtered {
            println!("    {} depth={:.1}dB prom={:.1}dB score={:.1}",
                     format_timestamp(v.position_seconds),
                     v.depth_db, v.prominence_db, v.score);
        }
        println!();
    }
    
    filtered
}

fn generate_cue_file(
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

fn write_cue_file(wav_file: &str, cue_content: &str) -> Result<PathBuf, std::io::Error> {
    let cue_path = Path::new(wav_file).with_extension("cue");
    let mut file = File::create(&cue_path)?;
    file.write_all(cue_content.as_bytes())?;
    Ok(cue_path)
}

fn has_cue_file(wav_file: &str) -> bool {
    Path::new(wav_file).with_extension("cue").exists()
}

fn collect_wav_files(directory: &str) -> Vec<PathBuf> {
    let mut wav_files = Vec::new();
    
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                wav_files.push(path);
            }
        }
    }
    
    wav_files.sort();
    wav_files
}

/// Guided boundary detection using expected track positions from MusicBrainz.
/// Searches for valleys within a window around each expected boundary.
fn find_guided_boundaries(
    smoothed: &[f32],
    timestamps: &[f64],
    expected_tracks: &[musicbrainz::ExpectedTrack],
    music_start: f64,
    search_window_seconds: f64,
    verbose: bool,
) -> Vec<Valley> {
    if expected_tracks.len() < 2 {
        return Vec::new();
    }
    
    let mut boundaries = Vec::new();
    
    // For each expected boundary between tracks
    for i in 1..expected_tracks.len() {
        let expected_pos = music_start + expected_tracks[i].expected_start;
        let window_start = expected_pos - search_window_seconds;
        let window_end = expected_pos + search_window_seconds;
        
        // Find the minimum RMS within the search window
        let mut min_rms = f32::MAX;
        let mut min_pos = expected_pos;
        let mut min_idx = 0;
        
        for (j, &ts) in timestamps.iter().enumerate() {
            if ts >= window_start && ts <= window_end && j < smoothed.len() {
                if smoothed[j] < min_rms {
                    min_rms = smoothed[j];
                    min_pos = ts;
                    min_idx = j;
                }
            }
        }
        
        if min_rms < f32::MAX {
            // Calculate prominence from surrounding context
            let context_window = 75; // ~15 seconds at 200ms chunks
            let left_start = min_idx.saturating_sub(context_window);
            let left_end = min_idx;
            let right_start = min_idx + 1;
            let right_end = (min_idx + context_window).min(smoothed.len());
            
            let left_avg = if left_end > left_start {
                smoothed[left_start..left_end].iter().sum::<f32>() / (left_end - left_start) as f32
            } else {
                min_rms
            };
            
            let right_avg = if right_end > right_start {
                smoothed[right_start..right_end].iter().sum::<f32>() / (right_end - right_start) as f32
            } else {
                min_rms
            };
            
            let prominence = (left_avg.max(right_avg) - min_rms).max(0.0);
            
            if verbose {
                println!("  Track {} boundary: expected={:.1}s, found={:.1}s (offset={:.1}s), depth={:.1}dB, prom={:.1}dB",
                         i + 1, expected_tracks[i].expected_start, min_pos - music_start,
                         min_pos - expected_pos, min_rms, prominence);
            }
            
            boundaries.push(Valley {
                position_seconds: min_pos,
                depth_db: min_rms,
                prominence_db: prominence,
                width_seconds: 0.0,
                left_level_db: left_avg,
                right_level_db: right_avg,
                score: (prominence * 10.0) as f64,
            });
        }
    }
    
    boundaries
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let dump = args.iter().any(|a| a == "--dump");
    let no_lookup = args.iter().any(|a| a == "--no-lookup");
    let no_cue = args.iter().any(|a| a == "--no-cue");
    
    let directory = args.iter()
        .position(|a| a == "--directory" || a == "-d")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());
    
    let min_prominence = args.iter()
        .position(|a| a == "--min-prominence")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(3.0);
    
    let min_song_duration = args.iter()
        .position(|a| a == "--min-song")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(30.0);
    
    let smooth_window_secs = args.iter()
        .position(|a| a == "--smooth-window")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(3.0);
    
    let chunk_ms = args.iter()
        .position(|a| a == "--chunk-ms")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(200);
    
    let option_flags = ["--min-prominence", "--min-song", "--smooth-window", "--chunk-ms", "--directory", "-d"];
    
    // Collect file arguments or process directory
    let mut wav_files_owned: Vec<PathBuf> = Vec::new();
    
    if let Some(dir) = directory {
        // Directory mode
        wav_files_owned = collect_wav_files(dir);
        if wav_files_owned.is_empty() {
            eprintln!("No WAV files found in directory: {}", dir);
            process::exit(1);
        }
    } else {
        // Individual file mode
        let file_args: Vec<&str> = args.iter()
            .skip(1)
            .filter(|a| !a.starts_with("--") && !a.starts_with("-"))
            .filter(|a| {
                let prev_idx = args.iter().position(|x| x == *a).unwrap();
                if prev_idx > 0 {
                    let prev = &args[prev_idx - 1];
                    if option_flags.contains(&prev.as_str()) {
                        return false;
                    }
                }
                true
            })
            .map(|s| s.as_str())
            .collect();
        
        wav_files_owned = file_args.iter().map(|s| PathBuf::from(s)).collect();
    }
    
    let wav_files: Vec<&str> = wav_files_owned.iter()
        .filter_map(|p| p.to_str())
        .collect();
    
    if wav_files.is_empty() {
        println!("Song Boundary Finder");
        println!("====================");
        println!();
        println!("Finds song boundaries in vinyl WAV recordings and generates CUE files.");
        println!("Automatically detects groove-in/groove-out and finds song transitions.");
        println!("Optionally looks up track names from MusicBrainz based on filename.");
        println!();
        println!("Usage: boundary_finder [OPTIONS] <FILE1.wav> [FILE2.wav ...]");
        println!("       boundary_finder [OPTIONS] --directory <DIR>");
        println!();
        println!("Options:");
        println!("  --verbose, -v            Show detailed analysis");
        println!("  --directory <DIR>, -d    Process all WAV files in directory");
        println!("  --dump                   Dump RMS curve (tab-separated, for plotting)");
        println!("  --no-lookup              Skip MusicBrainz release lookup");
        println!("  --no-cue                 Don't generate CUE files");
        println!("  --min-prominence <DB>    Minimum valley depth below local average (default: 3.0)");
        println!("  --min-song <SEC>         Minimum song duration in seconds (default: 30)");
        println!("  --smooth-window <SEC>    Smoothing window in seconds (default: 3.0)");
        println!("  --chunk-ms <MS>          RMS window size in milliseconds (default: 200)");
        println!();
        println!("Examples:");
        println!("  boundary_finder --verbose side_a.wav side_b.wav");
        println!("  boundary_finder --directory /music/at33ptg");
        println!();
        println!("Directory Mode:");
        println!("  - Processes all .wav files in the specified directory");
        println!("  - Skips files that already have .cue files");
        println!("  - Creates .cue files with detected boundaries and track info");
        process::exit(1);
    }
    
    // Directory mode: filter out files that already have .cue files
    let files_to_process: Vec<&str> = if directory.is_some() && !no_cue {
        let mut skipped = 0;
        let filtered: Vec<&str> = wav_files.iter()
            .filter(|f| {
                if has_cue_file(f) {
                    skipped += 1;
                    false
                } else {
                    true
                }
            })
            .copied()
            .collect();
        
        if skipped > 0 {
            println!("Skipping {} file(s) that already have .cue files", skipped);
            println!();
        }
        filtered
    } else {
        wav_files
    };
    
    if files_to_process.is_empty() {
        println!("No files to process (all files already have .cue files)");
        process::exit(0);
    }
    
    for wav_file in &files_to_process {
        if files_to_process.len() > 1 {
            println!();
            println!("{}", "=".repeat(60));
        }
        
        process_file(wav_file, verbose, dump, min_prominence, min_song_duration,
                     smooth_window_secs, chunk_ms, no_lookup, no_cue);
    }
}

fn process_file(
    wav_file: &str,
    verbose: bool,
    dump: bool,
    min_prominence_db: f32,
    min_song_duration: f64,
    smooth_window_secs: f64,
    chunk_ms: u32,
    no_lookup: bool,
    no_cue: bool,
) {
    if !Path::new(wav_file).exists() {
        eprintln!("Error: File not found: {}", wav_file);
        return;
    }
    
    println!("Song Boundary Finder");
    println!("====================");
    println!("File: {}", wav_file);
    println!();
    
    let file = File::open(wav_file).unwrap();
    let mut reader = BufReader::new(file);
    let header = read_wav_header(&mut reader).unwrap();
    
    let bytes_per_sample = (header.bits_per_sample / 8) as usize;
    let file_duration = header.data_size as f64
        / (header.sample_rate as f64 * header.num_channels as f64 * bytes_per_sample as f64);
    
    println!("WAV: {}Hz, {}ch, {}bit, duration: {} ({:.1}s)",
             header.sample_rate, header.num_channels, header.bits_per_sample,
             format_timestamp(file_duration), file_duration);
    println!();
    
    let format = match header.bits_per_sample {
        16 => SampleFormat::S16,
        32 => SampleFormat::S32,
        _ => {
            eprintln!("Error: Unsupported bit depth: {}", header.bits_per_sample);
            return;
        }
    };
    
    // ==== Pass 1: Compute RMS for entire file ====
    let chunk_samples = (header.sample_rate as f64 * chunk_ms as f64 / 1000.0) as usize;
    let chunk_bytes = chunk_samples * header.num_channels as usize * bytes_per_sample;
    let chunk_duration = chunk_ms as f64 / 1000.0;
    
    let mut rms_values: Vec<f32> = Vec::new();
    let mut timestamps: Vec<f64> = Vec::new();
    let mut position = 0.0_f64;
    
    if verbose {
        println!("Pass 1: Computing RMS ({}ms windows)...", chunk_ms);
    }
    
    loop {
        let mut buffer = vec![0u8; chunk_bytes];
        let bytes_read = reader.read(&mut buffer).unwrap_or(0);
        if bytes_read == 0 { break; }
        
        let samples_in_chunk = bytes_read / (header.num_channels as usize * bytes_per_sample);
        if samples_in_chunk == 0 { break; }
        
        let mut audio_data: Vec<Vec<i32>> =
            vec![Vec::with_capacity(samples_in_chunk); header.num_channels as usize];
        
        for i in 0..samples_in_chunk {
            for ch in 0..header.num_channels as usize {
                let off = (i * header.num_channels as usize + ch) * bytes_per_sample;
                if off + bytes_per_sample > bytes_read { break; }
                let sample = match format {
                    SampleFormat::S16 => {
                        i16::from_le_bytes([buffer[off], buffer[off + 1]]) as i32
                    }
                    SampleFormat::S32 => {
                        i32::from_le_bytes([buffer[off], buffer[off+1], buffer[off+2], buffer[off+3]])
                    }
                };
                audio_data[ch].push(sample);
            }
        }
        
        rms_values.push(compute_rms_db(&audio_data, format));
        timestamps.push(position);
        position += chunk_duration;
    }
    
    if verbose {
        println!("  {} RMS values over {:.1}s", rms_values.len(), position);
    }
    
    // ==== Smoothing ====
    let smooth_window = ((smooth_window_secs / chunk_duration) as usize).max(3) | 1;
    let smoothed = smooth_rms(&rms_values, smooth_window);
    
    // ==== Level estimates ====
    let noise_floor = estimate_noise_floor(&smoothed);
    let music_level = estimate_music_level(&smoothed);
    
    println!("Levels:");
    println!("  Noise floor: {:.1} dB (groove noise)", noise_floor);
    println!("  Music level: {:.1} dB (typical music)", music_level);
    println!("  Difference:  {:.1} dB", music_level - noise_floor);
    println!();
    
    // ==== Pass 2: Groove-in / Groove-out detection ====
    if verbose {
        println!("Pass 2: Detecting groove-in and groove-out...");
    }
    
    let groove_in = detect_groove_in(&smoothed, &timestamps, noise_floor, music_level,
                                      chunk_duration, verbose);
    let groove_out = detect_groove_out(&smoothed, &timestamps, noise_floor, music_level,
                                       file_duration, chunk_duration, verbose);
    let music_duration = groove_out - groove_in;
    
    println!("Music region:");
    println!("  Groove-in:  {} ({:.1}s lead-in)", format_timestamp(groove_in), groove_in);
    println!("  Groove-out: {} ({:.1}s lead-out)", format_timestamp(groove_out),
             file_duration - groove_out);
    println!("  Music:      {} ({:.1}s)", format_timestamp(music_duration), music_duration);
    println!();
    
    let music_start_idx = timestamps.iter().position(|&t| t >= groove_in).unwrap_or(0);
    let music_end_idx = timestamps.iter().position(|&t| t >= groove_out).unwrap_or(timestamps.len());
    
    // ==== MusicBrainz lookup ====
    let mut track_names: Vec<String> = Vec::new();
    let mut mb_info: Option<String> = None;
    let mut mb_tracks: Option<Vec<musicbrainz::ExpectedTrack>> = None;
    let mut use_guided_detection = false;
    
    if !no_lookup {
        println!("MusicBrainz Lookup:");
        println!("-------------------");
        match musicbrainz::auto_lookup_release(wav_file, music_duration, verbose) {
            Ok(Some(release)) => {
                println!("Found: {} - {}", release.artist, release.title);
                println!("Release ID: {}", release.release_id);
                println!("Format: {}", if release.is_vinyl { "Vinyl" } else { "Other" });
                println!("Tracks: {}", release.track_count);
                println!("URL: https://musicbrainz.org/release/{}", release.release_id);
                
                mb_info = Some(format!("{} - {} [{}]",
                                       release.artist, release.title, release.release_id));
                
                // Fetch track listing for this side
                if let Ok(all_tracks) = musicbrainz::fetch_release_info(&release.release_id) {
                    let (_, side_tracks) = musicbrainz::match_tracks_to_duration(&all_tracks, music_duration);
                    
                    // Check if duration match is good enough for guided detection (within 3%)
                    let expected_duration: f64 = side_tracks.iter().map(|t| t.length_seconds).sum();
                    let duration_error = (expected_duration - music_duration).abs();
                    let error_percent = (duration_error / music_duration) * 100.0;
                    
                    if error_percent <= 3.0 && side_tracks.len() >= 2 {
                        use_guided_detection = true;
                        mb_tracks = Some(side_tracks.clone());
                        if verbose {
                            println!("Duration match: {:.1}% error - using guided detection", error_percent);
                        }
                    } else if verbose {
                        println!("Duration match: {:.1}% error - using autonomous detection", error_percent);
                    }
                    
                    track_names = side_tracks.iter()
                        .map(|t| format!("#{} {}", t.position, t.title))
                        .collect();
                }
            }
            Ok(None) => {
                println!("No matching release found");
            }
            Err(e) => {
                if verbose {
                    println!("Lookup failed: {}", e);
                }
            }
        }
        println!();
    }
    
    // Dump mode
    if dump {
        println!("# timestamp_s\traw_rms_db\tsmoothed_rms_db\tin_music");
        for i in 0..rms_values.len() {
            let in_music = if i >= music_start_idx && i < music_end_idx { 1 } else { 0 };
            println!("{:.2}\t{:.2}\t{:.2}\t{}", timestamps[i], rms_values[i], smoothed[i], in_music);
        }
        println!();
    }
    
    // ==== Pass 3: Find song boundaries within music region ====
    let valleys = if use_guided_detection {
        if verbose {
            println!("Pass 3: Guided boundary detection (using MusicBrainz track positions)...");
        }
        let search_window = 10.0; // Search ±10 seconds around expected positions
        find_guided_boundaries(
            &smoothed, &timestamps,
            mb_tracks.as_ref().unwrap(),
            groove_in,
            search_window,
            verbose,
        )
    } else {
        if verbose {
            println!("Pass 3: Autonomous boundary detection (prominence >= {:.1} dB, min song {:.0}s)...",
                     min_prominence_db, min_song_duration);
        }
        find_song_boundaries(
            &rms_values, &timestamps, &smoothed,
            music_start_idx, music_end_idx,
            min_prominence_db, min_song_duration,
            chunk_duration, noise_floor, music_level, verbose,
        )
    };
    
    // ==== Results ====
    println!();
    println!("Results");
    println!("=======");
    if let Some(ref info) = mb_info {
        println!("Release: {}", info);
    }
    println!("Boundaries found: {}", valleys.len());
    println!("Songs detected: {}", valleys.len() + 1);
    println!();

    if valleys.is_empty() {
        println!("No song boundaries detected.");
        println!();
        println!("Tips:");
        println!("  - Try lowering --min-prominence (current: {:.1})", min_prominence_db);
        println!("  - Try lowering --min-song (current: {:.0})", min_song_duration);
        println!("  - Use --dump to visualise the RMS curve");
        println!("  - Use --verbose for more detail");
    } else {
        let mut prev_time = groove_in;
        for (i, valley) in valleys.iter().enumerate() {
            let song_dur = valley.position_seconds - prev_time;
            let name = track_names.get(i)
                .map(|n| format!(" - {}", n))
                .unwrap_or_default();
            println!("  Song {}: {} (starts @ {}){}",
                     i + 1, format_timestamp(song_dur), format_timestamp(prev_time), name);
            if verbose {
                println!("    --- boundary at {} [depth={:.1}dB prom={:.1}dB L={:.1}dB R={:.1}dB w={:.1}s score={:.1}]",
                         format_timestamp(valley.position_seconds),
                         valley.depth_db, valley.prominence_db,
                         valley.left_level_db, valley.right_level_db,
                         valley.width_seconds, valley.score);
            } else {
                println!("    --- boundary at {} ---",
                         format_timestamp(valley.position_seconds));
            }
            prev_time = valley.position_seconds;
        }

        let last_dur = groove_out - prev_time;
        let name = track_names.get(valleys.len())
            .map(|n| format!(" - {}", n))
            .unwrap_or_default();
        println!("  Song {}: {} (starts @ {}){}",
                 valleys.len() + 1, format_timestamp(last_dur), format_timestamp(prev_time), name);
    }
    println!();
    
    // ==== Generate CUE file ====
    if !no_cue && !valleys.is_empty() {
        let artist = mb_info.as_ref()
            .and_then(|info| info.split(" - ").next())
            .unwrap_or("Unknown Artist");
        
        let title = mb_info.as_ref()
            .and_then(|info| {
                let parts: Vec<&str> = info.split(" - ").collect();
                if parts.len() >= 2 {
                    parts[1].split(" [").next()
                } else {
                    None
                }
            })
            .unwrap_or("Unknown Album");
        
        let cue_content = generate_cue_file(wav_file, artist, title, &track_names, groove_in, &valleys);
        
        match write_cue_file(wav_file, &cue_content) {
            Ok(cue_path) => {
                println!("CUE file created: {}", cue_path.display());
            }
            Err(e) => {
                eprintln!("Warning: Failed to write CUE file: {}", e);
            }
        }
    }}