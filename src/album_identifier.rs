use std::process::Command;
use std::path::Path;
use serde::{Deserialize, Serialize};

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
    pub songs: Vec<IdentifiedSong>,
    pub confidence: f64,
}

/// Identify songs at specific timestamps in a WAV file using songrec
pub fn identify_songs_at_timestamps(wav_path: &str, timestamps: &[f64]) -> Result<Vec<IdentifiedSong>, String> {
    let path = Path::new(wav_path);
    if !path.exists() {
        return Err(format!("WAV file not found: {}", wav_path));
    }

    let mut identified_songs = Vec::new();

    for &timestamp in timestamps {
        println!("Identifying song at {}...", format_timestamp(timestamp));
        
        // Extract 30-second segment using ffmpeg (songrec doesn't support seeking)
        let temp_file = format!("/tmp/songrec_segment_{}.wav", timestamp as u32);
        
        let extract_result = Command::new("ffmpeg")
            .args(&[
                "-i", wav_path,
                "-ss", &timestamp.to_string(),
                "-t", "30",  // 30 seconds
                "-y",  // Overwrite output file
                &temp_file
            ])
            .stderr(std::process::Stdio::null())  // Hide ffmpeg output
            .status();
        
        if let Err(e) = extract_result {
            eprintln!("  Error extracting segment with ffmpeg: {}", e);
            continue;
        }
        
        // Run songrec on the extracted segment
        let output = Command::new("songrec")
            .arg("audio-file-to-recognized-song")
            .arg(&temp_file)
            .output();

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_file);

        match output {
            Ok(result) if result.status.success() => {
                let stdout = String::from_utf8_lossy(&result.stdout);
                
                // Parse songrec JSON output
                if let Ok(mut song_data) = parse_songrec_output(&stdout) {
                    song_data.timestamp = timestamp;
                    println!("  Found: {} - {}", song_data.artist, song_data.title);
                    identified_songs.push(song_data);
                } else {
                    println!("  No match found");
                }
            }
            Ok(result) => {
                eprintln!("  songrec failed: {}", String::from_utf8_lossy(&result.stderr));
            }
            Err(e) => {
                eprintln!("  Error running songrec: {}", e);
            }
        }
    }

    Ok(identified_songs)
}

/// Generate default timestamps: 1:00, then every 6 minutes
pub fn generate_default_timestamps(duration_seconds: f64) -> Vec<f64> {
    let mut timestamps = vec![60.0]; // Start at 1:00
    
    let mut current = 60.0 + 360.0; // Next at 1:00 + 6:00 = 7:00
    while current < duration_seconds - 30.0 { // Leave 30s margin at end
        timestamps.push(current);
        current += 360.0; // Add 6 minutes
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
    
    // Find most common album and artist
    let album_title = album_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(album, _)| album.clone())
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
        songs: songs.to_vec(),
        confidence,
    })
}

/// Format timestamp as MM:SS
fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0) as u32;
    let secs = (seconds % 60.0) as u32;
    format!("{}:{:02}", mins, secs)
}

/// Main function to identify album from a WAV file
pub fn identify_album(wav_path: &str, timestamps: Option<Vec<f64>>) -> Result<AlbumInfo, String> {
    // Get WAV duration if timestamps not provided
    let timestamps = if let Some(ts) = timestamps {
        ts
    } else {
        // For now, use a reasonable default (assume 30 minute recording)
        // In production, we should read the WAV header
        generate_default_timestamps(1800.0)
    };
    
    println!("Identifying songs in: {}", wav_path);
    println!("Using {} timestamp(s)", timestamps.len());
    println!();
    
    // Identify songs at each timestamp
    let mut songs = identify_songs_at_timestamps(wav_path, &timestamps)?;
    
    if songs.is_empty() {
        return Err("No songs could be identified".to_string());
    }
    
    println!();
    println!("Found {} song(s)", songs.len());
    println!();
    
    // Identify album from songs
    let album_info = identify_album_from_songs(&songs)?;
    
    println!("Album: {} - {}", album_info.album_artist, album_info.album_title);
    println!("Confidence: {:.0}%", album_info.confidence * 100.0);
    
    Ok(album_info)
}
