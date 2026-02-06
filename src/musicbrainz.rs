//! MusicBrainz-guided detection - uses expected track lengths to find boundaries.

use serde::Deserialize;
use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::Duration as StdDuration;

#[derive(Debug, Deserialize)]
struct MusicBrainzRelease {
    media: Vec<Medium>,
}

#[derive(Debug, Deserialize)]
struct Medium {
    tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Track {
    title: String,
    length: Option<u64>,  // in milliseconds
    position: u32,
}

// Search API response types
#[derive(Debug, Deserialize)]
struct SearchResponse {
    releases: Vec<SearchRelease>,
}

#[derive(Debug, Deserialize)]
struct SearchRelease {
    id: String,
    score: u32,
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    #[serde(default)]
    media: Vec<SearchMedium>,
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ArtistCredit {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SearchMedium {
    format: Option<String>,
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub release_id: String,
    pub title: String,
    pub artist: String,
    pub score: u32,
    pub is_vinyl: bool,
    pub track_count: u32,
}

#[derive(Debug, Clone)]
pub struct ExpectedTrack {
    pub position: u32,
    pub title: String,
    pub length_seconds: f64,
    pub expected_start: f64,
}

pub fn fetch_release_info(release_id: &str) -> Result<Vec<ExpectedTrack>, Box<dyn Error>> {
    let url = format!(
        "https://musicbrainz.org/ws/2/release/{}?inc=recordings&fmt=json",
        release_id
    );
    
    // MusicBrainz requires a User-Agent
    let response = ureq::get(&url)
        .set("User-Agent", "HiFiBerryAutoRec/0.1 (https://github.com/hifiberry/autorec)")
        .call()?;
    
    let release: MusicBrainzRelease = serde_json::from_reader(response.into_reader())?;
    
    let mut tracks = Vec::new();
    let mut cumulative_time = 0.0;
    
    // Get tracks from first medium (assuming single disc)
    if let Some(medium) = release.media.first() {
        for track in &medium.tracks {
            if let Some(length_ms) = track.length {
                let length_seconds = length_ms as f64 / 1000.0;
                
                tracks.push(ExpectedTrack {
                    position: track.position,
                    title: track.title.clone(),
                    length_seconds,
                    expected_start: cumulative_time,
                });
                
                cumulative_time += length_seconds;
            }
        }
    }
    
    Ok(tracks)
}

pub fn parse_musicbrainz_url(url: &str) -> Option<String> {
    // Extract release ID from URLs like:
    // https://musicbrainz.org/release/768a1c5f-3657-4e29-aac4-c1de6ee5221f
    if let Some(idx) = url.rfind("/release/") {
        let id_start = idx + 9;
        let id = &url[id_start..];
        // Remove any query params or fragments
        let id = id.split('?').next().unwrap_or(id);
        let id = id.split('#').next().unwrap_or(id);
        Some(id.to_string())
    } else {
        // Maybe it's just the ID
        if url.len() == 36 && url.contains('-') {
            Some(url.to_string())
        } else {
            None
        }
    }
}

/// Determine which tracks from a release belong to this file based on duration.
/// Returns (track_offset, filtered_tracks) where track_offset is the 0-based index
/// of the first track in this file.
///
/// This handles multi-file vinyl releases where files represent different sides.
/// The algorithm finds the best split point that divides the album, then determines
/// which side this file represents based on duration matching.
pub fn match_tracks_to_duration(all_tracks: &[ExpectedTrack], file_duration_seconds: f64) -> (usize, Vec<ExpectedTrack>) {
    if all_tracks.is_empty() {
        return (0, Vec::new());
    }
    
    let total_duration: f64 = all_tracks.iter().map(|t| t.length_seconds).sum();
    
    // If file duration is close to total duration (within 20%), assume it's a single file
    if (file_duration_seconds - total_duration).abs() / total_duration < 0.2 {
        return (0, all_tracks.to_vec());
    }
    
    // Try to find the best split point where cumulative duration matches file duration
    let mut best_offset = 0;
    let mut best_diff = f64::MAX;
    
    for split_point in 1..all_tracks.len() {
        // Calculate duration of tracks 0..split_point (side A)
        let side_a_duration: f64 = all_tracks[0..split_point].iter().map(|t| t.length_seconds).sum();
        let side_a_diff = (file_duration_seconds - side_a_duration).abs();
        
        // Calculate duration of tracks split_point.. (side B)
        let side_b_duration: f64 = all_tracks[split_point..].iter().map(|t| t.length_seconds).sum();
        let side_b_diff = (file_duration_seconds - side_b_duration).abs();
        
        // Find the split that gives the best match for EITHER side
        let min_diff = side_a_diff.min(side_b_diff);
        if min_diff < best_diff {
            best_diff = min_diff;
            best_offset = split_point;
        }
    }
    
    // Now determine if this file is side A or side B using the best split point
    let side_a_duration: f64 = all_tracks[0..best_offset].iter().map(|t| t.length_seconds).sum();
    let side_b_duration: f64 = all_tracks[best_offset..].iter().map(|t| t.length_seconds).sum();
    
    let side_a_diff = (file_duration_seconds - side_a_duration).abs();
    let side_b_diff = (file_duration_seconds - side_b_duration).abs();
    
    let is_side_a = side_a_diff < side_b_diff;
    
    if is_side_a {
        // Side A: tracks 0..best_offset
        let filtered = all_tracks[0..best_offset].to_vec();
        (0, filtered)
    } else {
        // Side B: tracks best_offset.., adjust expected_start to be relative to start of this file
        let mut filtered: Vec<ExpectedTrack> = all_tracks[best_offset..].to_vec();
        let offset_time = side_a_duration;
        
        for track in &mut filtered {
            track.expected_start -= offset_time;
        }
        
        (best_offset, filtered)
    }
}

/// Parse a recording filename to extract word parts and side number.
/// E.g. "/music/at33ptg/kanonenfieber_soldatenschicksale.1.wav" → (["kanonenfieber", "soldatenschicksale"], 1)
/// E.g. "/music/at33ptg/dj_shadow_endtroducing.2.wav" → (["dj", "shadow", "endtroducing"], 2)
pub fn parse_recording_filename(path: &str) -> Option<(Vec<String>, u32)> {
    let filename = Path::new(path).file_name()?.to_str()?;

    // Strip .wav extension
    let without_ext = filename.strip_suffix(".wav")
        .or_else(|| filename.strip_suffix(".WAV"))?;

    // Split off the side number: "kanonenfieber_soldatenschicksale.1" → ("kanonenfieber_soldatenschicksale", "1")
    let (base, side_str) = without_ext.rsplit_once('.')?;
    let side: u32 = side_str.parse().ok()?;

    let words: Vec<String> = base.split('_').map(|s| s.to_string()).collect();
    if words.len() < 2 {
        return None; // Need at least one word for artist and one for release
    }

    Some((words, side))
}

/// Search MusicBrainz for a release by artist and release name.
/// Returns up to `limit` results sorted by score.
fn search_release(artist: &str, release: &str, limit: u32) -> Result<Vec<SearchResult>, Box<dyn Error>> {
    // URL-encode the query by replacing spaces with +
    let artist_q = artist.replace(' ', "+");
    let release_q = release.replace(' ', "+");

    let url = format!(
        "https://musicbrainz.org/ws/2/release/?query=artist:{}+release:{}&fmt=json&limit={}",
        artist_q, release_q, limit
    );

    let response = ureq::get(&url)
        .set("User-Agent", "HiFiBerryAutoRec/0.1 (https://github.com/hifiberry/autorec)")
        .call()?;

    let search: SearchResponse = serde_json::from_reader(response.into_reader())?;

    let mut results = Vec::new();
    for r in search.releases {
        let artist_name = r.artist_credit.first()
            .map(|ac| ac.name.clone())
            .unwrap_or_default();

        let is_vinyl = r.media.iter().any(|m| {
            m.format.as_deref().map_or(false, |f| f.contains("Vinyl"))
        });

        let track_count = r.track_count.unwrap_or(0);

        results.push(SearchResult {
            release_id: r.id,
            title: r.title,
            artist: artist_name,
            score: r.score,
            is_vinyl,
            track_count,
        });
    }

    Ok(results)
}

/// Search MusicBrainz by trying all possible artist/release splits of the filename words.
/// E.g. for ["dj", "shadow", "endtroducing"], tries:
///   - artist="dj", release="shadow endtroducing"
///   - artist="dj shadow", release="endtroducing"
/// Returns all matching results (score >= 80) from all splits.
pub fn search_release_by_filename(words: &[String], verbose: bool) -> Result<Vec<SearchResult>, Box<dyn Error>> {
    if words.len() < 2 {
        return Ok(Vec::new());
    }

    let mut all_results = Vec::new();

    for split in 1..words.len() {
        let artist = words[..split].join(" ");
        let release = words[split..].join(" ");

        if verbose {
            eprintln!("  Searching: artist=\"{}\" release=\"{}\"", artist, release);
        }

        match search_release(&artist, &release, 5) {
            Ok(results) => {
                for r in results {
                    if r.score < 80 {
                        continue;
                    }
                    all_results.push(r);
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!("  Search failed: {}", e);
                }
            }
        }

        // MusicBrainz rate limit: 1 request per second
        if split < words.len() - 1 {
            thread::sleep(StdDuration::from_millis(1100));
        }
    }

    // Deduplicate by release_id
    let mut seen_ids = std::collections::HashSet::new();
    all_results.retain(|r| seen_ids.insert(r.release_id.clone()));

    Ok(all_results)
}

/// Rank search results by how well their total duration matches the music duration.
/// For multi-file vinyl (detected by checking if total duration >> music duration),
/// finds the best side split and ranks by that side's match quality.
pub fn rank_by_duration_match(
    results: &[SearchResult],
    music_duration_seconds: f64,
    verbose: bool,
) -> Result<Vec<(SearchResult, f64)>, Box<dyn Error>> {
    let mut ranked = Vec::new();

    for result in results {
        // Fetch full track info
        let tracks = match fetch_release_info(&result.release_id) {
            Ok(t) => t,
            Err(e) => {
                if verbose {
                    eprintln!("  Failed to fetch tracks for {}: {}", result.release_id, e);
                }
                continue;
            }
        };

        if tracks.is_empty() {
            continue;
        }

        let total_duration: f64 = tracks.iter().map(|t| t.length_seconds).sum();

        // Check if this is likely a multi-side vinyl release
        // If total duration is much larger than our music duration, find best side match
        let duration_ratio = total_duration / music_duration_seconds;
        
        let match_error = if duration_ratio > 1.5 {
            // Multi-side vinyl: find best split
            let (_, side_tracks) = match_tracks_to_duration(&tracks, music_duration_seconds);
            let side_duration: f64 = side_tracks.iter().map(|t| t.length_seconds).sum();
            (side_duration - music_duration_seconds).abs()
        } else {
            // Single file or close match
            (total_duration - music_duration_seconds).abs()
        };

        ranked.push((result.clone(), match_error));

        // MusicBrainz rate limit
        thread::sleep(StdDuration::from_millis(1100));
    }

    // Sort by match error (lower is better)
    ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    Ok(ranked)
}

/// Automatic release lookup from filename using music duration for ranking.
/// Returns the best matching release based on filename parsing and duration match.
pub fn auto_lookup_release(
    filepath: &str,
    music_duration_seconds: f64,
    verbose: bool,
) -> Result<Option<SearchResult>, Box<dyn Error>> {
    // Parse filename
    let (words, side) = match parse_recording_filename(filepath) {
        Some((w, s)) => (w, s),
        None => {
            if verbose {
                eprintln!("Could not parse filename: {}", filepath);
            }
            return Ok(None);
        }
    };

    if verbose {
        println!("Parsed filename: words={:?}, side={}", words, side);
        println!("Music duration (without grooves): {:.1}s", music_duration_seconds);
        println!();
    }

    // Search for all matching releases
    let search_results = search_release_by_filename(&words, verbose)?;
    
    if search_results.is_empty() {
        if verbose {
            println!("No MusicBrainz matches found");
        }
        return Ok(None);
    }

    if verbose {
        println!();
        println!("Found {} potential matches, ranking by duration...", search_results.len());
    }

    // Rank all results by duration match
    let ranked = rank_by_duration_match(&search_results, music_duration_seconds, verbose)?;

    if ranked.is_empty() {
        return Ok(None);
    }

    let (best, error) = &ranked[0];
    
    if verbose {
        println!();
        println!("Best match: {} - {} (score: {}, vinyl: {})",
                 best.artist, best.title, best.score, best.is_vinyl);
        println!("Release ID: {}", best.release_id);
        println!("Duration match error: {:.1}s", error);
    }

    // Accept if error is within 5% or 30 seconds (whichever is larger)
    let threshold = (music_duration_seconds * 0.05).max(30.0);
    if *error > threshold {
        if verbose {
            println!("Duration mismatch too large (threshold: {:.1}s)", threshold);
        }
        return Ok(None);
    }

    Ok(Some(best.clone()))
}
