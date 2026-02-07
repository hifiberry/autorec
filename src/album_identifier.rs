use std::process::Command;
use std::path::Path;
use std::time::Duration;
use std::thread;
use serde::{Deserialize, Serialize};
use crate::wavfile::{extract_wav_segment, read_wav_header};
use crate::songrec_cache;
use crate::rate_limiter::RateLimiter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentifiedSong {
    pub timestamp: f64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

/// Result from song identification including log
pub struct IdentificationResult {
    pub songs: Vec<IdentifiedSong>,
    pub log: String,
}

/// Identify songs at specific timestamps in a WAV file using songrec
pub fn identify_songs_at_timestamps(wav_path: &str, timestamps: &[f64]) -> Result<IdentificationResult, String> {
    let path = Path::new(wav_path);
    if !path.exists() {
        return Err(format!("WAV file not found: {}", wav_path));
    }

    let mut identified_songs = Vec::new();
    let mut rate_limiter = RateLimiter::from_secs("songrec", 5);
    let mut log = String::new();

    // Load songrec cache
    let mut cache = songrec_cache::load_cache();
    let cache_size = cache.len();
    if cache_size > 0 {
        let msg = format!("Loaded songrec cache with {} entries", cache_size);
        println!("{}", msg);
        log.push_str(&msg);
        log.push('\n');
    }

    for &timestamp in timestamps {
        let msg = format!("Identifying song at {}...", format_timestamp(timestamp));
        println!("{}", msg);
        log.push_str(&msg);
        log.push('\n');
        
        // Extract 30-second segment using native WAV extraction
        let temp_file = format!("/tmp/songrec_segment_{}.wav", timestamp as u32);
        
        if let Err(e) = extract_wav_segment(wav_path, &temp_file, timestamp, 30.0) {
            let msg = format!("  Error extracting segment: {}", e);
            eprintln!("{}", msg);
            log.push_str(&msg);
            log.push('\n');
            continue;
        }
        
        // Check cache before calling songrec
        let cache_key = songrec_cache::cache_key(&temp_file);
        if let Some(ref key) = cache_key {
            if let Some(cached_json) = cache.get(key) {
                let msg = "  Cache hit, skipping songrec API call";
                println!("{}", msg);
                log.push_str(msg);
                log.push('\n');
                if let Ok(mut song_data) = parse_songrec_output(cached_json) {
                    song_data.timestamp = timestamp;
                    let msg = format!("  Found: {} - {}", song_data.artist, song_data.title);
                    println!("{}", msg);
                    log.push_str(&msg);
                    log.push('\n');
                    identified_songs.push(song_data);
                } else {
                    let msg = "  Cached result: no match";
                    println!("{}", msg);
                    log.push_str(msg);
                    log.push('\n');
                }
                let _ = std::fs::remove_file(&temp_file);
                continue;
            }
        }

        // Apply rate limiting before making the request
        rate_limiter.wait_if_needed();
        
        // Run songrec on the extracted segment
        let output = Command::new("songrec")
            .arg("audio-file-to-recognized-song")
            .arg(&temp_file)
            .output();

        match output {
            Ok(result) if result.status.success() => {
                let stdout = String::from_utf8_lossy(&result.stdout).to_string();
                
                // Store in cache
                if let Some(ref key) = cache_key {
                    songrec_cache::append_to_cache(key, &stdout);
                    cache.insert(key.clone(), stdout.clone());
                }
                
                // Parse songrec JSON output
                if let Ok(mut song_data) = parse_songrec_output(&stdout) {
                    song_data.timestamp = timestamp;
                    let msg = format!("  Found: {} - {}", song_data.artist, song_data.title);
                    println!("{}", msg);
                    log.push_str(&msg);
                    log.push('\n');
                    rate_limiter.report_success();
                    identified_songs.push(song_data);
                } else {
                    let msg = "  No match found";
                    println!("{}", msg);
                    log.push_str(msg);
                    log.push('\n');
                    rate_limiter.report_success();
                }
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                let msg = format!("  songrec failed: {}", stderr);
                eprintln!("{}", msg);
                log.push_str(&msg);
                log.push('\n');
                
                // Check if it's a decode error (rate limiting issue)
                if stderr.contains("Decode") || stderr.contains("expected value") {
                    let msg = "  Retrying after 30s wait...";
                    println!("{}", msg);
                    log.push_str(msg);
                    log.push('\n');
                    thread::sleep(Duration::from_secs(30));
                    
                    let retry_output = Command::new("songrec")
                        .arg("audio-file-to-recognized-song")
                        .arg(&temp_file)
                        .output();
                    
                    match retry_output {
                        Ok(retry_result) if retry_result.status.success() => {
                            let stdout = String::from_utf8_lossy(&retry_result.stdout).to_string();
                            
                            // Store in cache
                            if let Some(ref key) = cache_key {
                                songrec_cache::append_to_cache(key, &stdout);
                                cache.insert(key.clone(), stdout.clone());
                            }
                            
                            if let Ok(mut song_data) = parse_songrec_output(&stdout) {
                                song_data.timestamp = timestamp;
                                let msg = format!("  Retry succeeded: {} - {}", song_data.artist, song_data.title);
                                println!("{}", msg);
                                log.push_str(&msg);
                                log.push('\n');
                                // Still increase rate limit since original request failed
                                rate_limiter.report_failure();
                                identified_songs.push(song_data);
                            } else {
                                let msg = "  Retry: no match found";
                                println!("{}", msg);
                                log.push_str(msg);
                                log.push('\n');
                                rate_limiter.report_failure();  // Original request failed
                            }
                        }
                        _ => {
                            let msg = "  Retry also failed, increasing rate limit";
                            eprintln!("{}", msg);
                            log.push_str(msg);
                            log.push('\n');
                            rate_limiter.report_failure();
                        }
                    }
                } else {
                    rate_limiter.report_success();
                }
            }
            Err(e) => {
                let msg = format!("  Error running songrec: {}", e);
                eprintln!("{}", msg);
                log.push_str(&msg);
                log.push('\n');
                rate_limiter.report_success();
            }
        }

        // Clean up temp file (after potential retry)
        let _ = std::fs::remove_file(&temp_file);
    }

    Ok(IdentificationResult { songs: identified_songs, log })
}

/// Generate default timestamps with configurable first timestamp and interval
pub fn generate_default_timestamps(duration_seconds: f64, first_seconds: f64, interval_seconds: f64) -> Vec<f64> {
    let mut timestamps = vec![first_seconds]; // Start at first_seconds
    
    let mut current = first_seconds + interval_seconds;
    while current < duration_seconds - 30.0 { // Leave 30s margin at end
        timestamps.push(current);
        current += interval_seconds;
    }
    
    timestamps
}

/// Parse songrec JSON output
fn parse_songrec_output(json_str: &str) -> Result<IdentifiedSong, String> {
    // songrec outputs JSON with track info
    let json: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse JSON: {}", e))?;
    
    let track = json.get("track")
        .ok_or_else(|| "No track info in response".to_string())?;
    
    let title = track.get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No title found".to_string())?
        .to_string();
    
    let artist = track.get("subtitle")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No artist found".to_string())?
        .to_string();
    
    // Try to get album info if available
    let album = track.get("sections")
        .and_then(|sections| sections.as_array())
        .and_then(|arr| {
            arr.iter().find(|section| {
                section.get("type").and_then(|t| t.as_str()) == Some("SONG")
            })
        })
        .and_then(|section| section.get("metadata"))
        .and_then(|metadata| metadata.as_array())
        .and_then(|arr| {
            arr.iter().find(|item| {
                item.get("title").and_then(|t| t.as_str()) == Some("Album")
            })
        })
        .and_then(|item| item.get("text"))
        .and_then(|text| text.as_str())
        .map(|s| s.to_string());
    
    Ok(IdentifiedSong {
        timestamp: 0.0, // Will be set by caller
        title,
        artist,
        album,
    })
}

/// Format timestamp as MM:SS
fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0) as u32;
    let secs = (seconds % 60.0) as u32;
    format!("{}:{:02}", mins, secs)
}

/// Main function to identify songs in a WAV file using Shazam/songrec
/// Returns (Result<Vec<IdentifiedSong>>, log_string) - log is always available even on error
pub fn identify_songs(wav_path: &str, timestamps: Option<Vec<f64>>) -> (Result<Vec<IdentifiedSong>, String>, String) {
    let mut log = String::new();
    
    // Get WAV duration if timestamps not provided
    let timestamps = if let Some(ts) = timestamps {
        ts
    } else {
        // Read actual file duration from WAV header
        let duration = match std::fs::File::open(wav_path) {
            Ok(f) => {
                let mut reader = std::io::BufReader::new(f);
                match read_wav_header(&mut reader) {
                    Ok(header) => {
                        let bytes_per_sample = (header.bits_per_sample / 8) as f64;
                        let frame_size = bytes_per_sample * header.num_channels as f64;
                        let dur = header.data_size as f64 / (header.sample_rate as f64 * frame_size);
                        if dur < 10.0 {
                            let msg = format!("WAV file too short ({:.1}s), skipping identification", dur);
                            log.push_str(&msg);
                            log.push('\n');
                            return (Err(msg), log);
                        }
                        dur
                    }
                    Err(e) => {
                        let msg = format!("Failed to read WAV header: {}", e);
                        log.push_str(&msg);
                        log.push('\n');
                        return (Err(msg), log);
                    }
                }
            }
            Err(e) => {
                let msg = format!("Failed to open WAV file: {}", e);
                log.push_str(&msg);
                log.push('\n');
                return (Err(msg), log);
            }
        };
        // Default: first at 1 min (60s), then every 2 mins (120s)
        generate_default_timestamps(duration, 60.0, 120.0)
    };
    
    let msg = format!("Identifying songs in: {}", wav_path);
    println!("{}", msg);
    log.push_str(&msg);
    log.push('\n');
    let msg = format!("Using {} timestamp(s)", timestamps.len());
    println!("{}", msg);
    log.push_str(&msg);
    log.push('\n');
    log.push('\n');
    println!();
    
    // Identify songs at each timestamp
    let id_result = match identify_songs_at_timestamps(wav_path, &timestamps) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Error: {}", e);
            log.push_str(&msg);
            log.push('\n');
            return (Err(e), log);
        }
    };
    
    log.push_str(&id_result.log);
    let songs = id_result.songs;
    
    if songs.is_empty() {
        let msg = "No songs could be identified".to_string();
        log.push_str(&msg);
        log.push('\n');
        return (Err(msg), log);
    }

    // Deduplicate consecutive identical songs (same artist + title).
    // Keep the first occurrence's timestamp for each run.
    let mut deduped: Vec<IdentifiedSong> = Vec::new();
    for song in &songs {
        let dominated = deduped.last().map_or(false, |prev| {
            prev.artist.eq_ignore_ascii_case(&song.artist)
                && prev.title.eq_ignore_ascii_case(&song.title)
        });
        if !dominated {
            deduped.push(song.clone());
        }
    }
    
    let msg = format!("\nFound {} song(s) ({} unique)", songs.len(), deduped.len());
    println!("{}", msg);
    log.push_str(&msg);
    log.push('\n');
    
    for song in &deduped {
        let msg = format!("  {} - {}", song.artist, song.title);
        println!("{}", msg);
        log.push_str(&msg);
        log.push('\n');
    }
    
    (Ok(deduped), log)
}
