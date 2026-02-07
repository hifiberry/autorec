use std::process::Command;
use std::path::Path;
use std::time::{Duration, Instant};
use std::thread;
use serde::{Deserialize, Serialize};
use crate::wavfile::{extract_wav_segment, read_wav_header};

/// Rate limiter for songrec API calls with adaptive backoff
struct RateLimiter {
    last_request: Option<Instant>,
    current_interval: Duration,
    base_interval: Duration,
    max_interval: Duration,
    success_count: u32,
}

impl RateLimiter {
    fn new(min_interval_secs: u64) -> Self {
        let base = Duration::from_secs(min_interval_secs);
        RateLimiter {
            last_request: None,
            current_interval: base,
            base_interval: base,
            max_interval: Duration::from_secs(min_interval_secs * 16), // Max 16x base
            success_count: 0,
        }
    }
    
    fn wait_if_needed(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.current_interval {
                let wait_time = self.current_interval - elapsed;
                println!("  Rate limiting: waiting {:.1}s before next request...", wait_time.as_secs_f64());
                thread::sleep(wait_time);
            }
        }
        self.last_request = Some(Instant::now());
    }
    
    fn report_success(&mut self) {
        self.success_count += 1;
        
        // After 10 successful requests, try reducing the interval
        if self.success_count >= 10 && self.current_interval > self.base_interval {
            let new_interval = self.current_interval / 2;
            if new_interval >= self.base_interval {
                self.current_interval = new_interval;
                println!("  Rate limit reduced to {:.1}s after {} successful requests", 
                         self.current_interval.as_secs_f64(), self.success_count);
            } else {
                self.current_interval = self.base_interval;
                println!("  Rate limit restored to {:.1}s after {} successful requests", 
                         self.current_interval.as_secs_f64(), self.success_count);
            }
            self.success_count = 0;
        }
    }
    
    fn report_failure(&mut self) {
        let new_interval = self.current_interval * 2;
        if new_interval <= self.max_interval {
            self.current_interval = new_interval;
        } else {
            self.current_interval = self.max_interval;
        }
        println!("  Rate limit increased to {:.1}s due to API error", 
                 self.current_interval.as_secs_f64());
        self.success_count = 0; // Reset success counter on failure
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentifiedSong {
    pub timestamp: f64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumInfo {
    pub album_title: String,
    pub album_artist: String,
    pub album_candidates: Vec<String>,
    pub songs: Vec<IdentifiedSong>,
    pub confidence: f64,
    #[serde(skip)]
    pub log: String,
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
    let mut rate_limiter = RateLimiter::new(5); // 5 seconds between requests
    let mut log = String::new();

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
        
        // Apply rate limiting before making the request
        rate_limiter.wait_if_needed();
        
        // Run songrec on the extracted segment
        let output = Command::new("songrec")
            .arg("audio-file-to-recognized-song")
            .arg(&temp_file)
            .output();

        match output {
            Ok(result) if result.status.success() => {
                let stdout = String::from_utf8_lossy(&result.stdout);
                
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
                            let stdout = String::from_utf8_lossy(&retry_result.stdout);
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

/// Query MusicBrainz to identify the album based on identified songs
pub fn identify_album_from_songs(songs: &[IdentifiedSong]) -> Result<AlbumInfo, String> {
    if songs.is_empty() {
        return Err("No songs to identify album from".to_string());
    }

    // For now, use the most common album name from the identified songs
    // In the future, we could query MusicBrainz API for more accurate results
    
    let mut album_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut artist_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    
    for song in songs {
        if let Some(ref album) = song.album {
            *album_counts.entry(album.clone()).or_insert(0) += 1;
        }
        *artist_counts.entry(song.artist.clone()).or_insert(0) += 1;
    }
    
    // Collect all unique album candidates sorted by frequency (most common first)
    let mut album_candidates_counted: Vec<(String, usize)> = album_counts.into_iter().collect();
    album_candidates_counted.sort_by(|a, b| b.1.cmp(&a.1));
    let album_candidates: Vec<String> = album_candidates_counted.into_iter().map(|(name, _)| name).collect();
    
    // Most common album is the first candidate
    let album_title = album_candidates.first()
        .cloned()
        .unwrap_or_else(|| "Unknown Album".to_string());
    
    let album_artist = artist_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(artist, _)| artist.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());
    
    // Calculate confidence based on consistency
    let max_album_count = album_counts.values().max().copied().unwrap_or(0);
    let confidence = if songs.is_empty() {
        0.0
    } else {
        max_album_count as f64 / songs.len() as f64
    };
    
    Ok(AlbumInfo {
        album_title,
        album_artist,
        album_candidates,
        songs: songs.to_vec(),
        confidence,
        log: String::new(),
    })
}

/// Format timestamp as MM:SS
fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0) as u32;
    let secs = (seconds % 60.0) as u32;
    format!("{}:{:02}", mins, secs)
}

/// Main function to identify album from a WAV file
/// Returns (Result<AlbumInfo>, log_string) - log is always available even on error
pub fn identify_album(wav_path: &str, timestamps: Option<Vec<f64>>) -> (Result<AlbumInfo, String>, String) {
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
        // Default: first at 1 min (60s), then every 4 mins (240s)
        generate_default_timestamps(duration, 60.0, 240.0)
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
    
    let msg = format!("\nFound {} song(s)\n", songs.len());
    println!("{}", msg);
    log.push_str(&msg);
    log.push('\n');
    
    // Identify album from songs
    let album_info = match identify_album_from_songs(&songs) {
        Ok(info) => info,
        Err(e) => {
            log.push_str(&format!("Error: {}\n", e));
            return (Err(e), log);
        }
    };
    
    let msg = format!("Album: {} - {}", album_info.album_artist, album_info.album_title);
    println!("{}", msg);
    log.push_str(&msg);
    log.push('\n');
    let msg = format!("Confidence: {:.0}%", album_info.confidence * 100.0);
    println!("{}", msg);
    log.push_str(&msg);
    log.push('\n');
    
    let mut result = album_info;
    result.log = log.clone();
    (Ok(result), log)
}
