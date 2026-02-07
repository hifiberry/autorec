//! MusicBrainz-guided detection - uses expected track lengths to find boundaries.

use serde::Deserialize;
use std::error::Error;
use std::path::Path;

use crate::album_identifier::IdentifiedSong;
use crate::rate_limiter::RateLimiter;

#[derive(Debug, Deserialize)]
struct MusicBrainzRelease {
    media: Vec<Medium>,
}

#[derive(Debug, Deserialize)]
struct Medium {
    position: u32,
    #[serde(default)]
    format: Option<String>,
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

// Recording search API response types
#[derive(Debug, Deserialize)]
struct RecordingSearchResponse {
    recordings: Vec<RecordingResult>,
}

#[derive(Debug, Deserialize)]
struct RecordingResult {
    #[allow(dead_code)]
    id: String,
    score: u32,
    #[allow(dead_code)]
    title: String,
    #[serde(default)]
    releases: Vec<RecordingRelease>,
}

#[derive(Debug, Deserialize)]
struct RecordingRelease {
    id: String,
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    #[serde(default)]
    media: Vec<SearchMedium>,
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

/// Information about a single medium (side) of a release
#[derive(Debug, Clone)]
pub struct MediumInfo {
    pub position: u32,
    pub format: Option<String>,
    pub tracks: Vec<ExpectedTrack>,
    pub total_duration: f64,
}

/// Fetch all sides/media of a release with per-side track listings.
pub fn fetch_release_sides(release_id: &str) -> Result<Vec<MediumInfo>, Box<dyn Error>> {
    let url = format!(
        "https://musicbrainz.org/ws/2/release/{}?inc=recordings&fmt=json",
        release_id
    );
    
    let response = ureq::get(&url)
        .set("User-Agent", "HiFiBerryAutoRec/0.1 (https://github.com/hifiberry/autorec)")
        .call()?;
    
    let release: MusicBrainzRelease = serde_json::from_reader(response.into_reader())?;
    
    let mut sides = Vec::new();
    
    for medium in &release.media {
        let mut tracks = Vec::new();
        let mut cumulative_time = 0.0;
        
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
        
        sides.push(MediumInfo {
            position: medium.position,
            format: medium.format.clone(),
            tracks,
            total_duration: cumulative_time,
        });
    }
    
    Ok(sides)
}

/// Fetch all tracks from a release as a flat list (legacy, uses first medium only).
pub fn fetch_release_info(release_id: &str) -> Result<Vec<ExpectedTrack>, Box<dyn Error>> {
    let sides = fetch_release_sides(release_id)?;
    if let Some(first) = sides.first() {
        Ok(first.tracks.clone())
    } else {
        Ok(Vec::new())
    }
}

/// Find the best matching side for a given file duration and identified songs.
/// For vinyl, each MusicBrainz medium is typically one disc (with 2 physical sides).
/// This function tries:
/// 1. Each medium as a whole (for single-side or CD releases)
/// 2. Splitting each medium's tracks by duration (for vinyl discs with 2 physical sides)
/// Ranks candidates by duration match AND overlap with identified song titles.
/// Returns the tracks for the best matching side with expected_start relative to start.
pub fn find_best_side(sides: &[MediumInfo], file_duration_seconds: f64, song_titles: &[String]) -> Option<Vec<ExpectedTrack>> {
    if sides.is_empty() {
        return None;
    }
    
    // If only one side and close to file duration, return it directly
    if sides.len() == 1 {
        let ratio = sides[0].total_duration / file_duration_seconds;
        if ratio < 1.5 {
            return Some(sides[0].tracks.clone());
        }
        // Otherwise try splitting this medium into sub-sides
        let (_, split_tracks) = match_tracks_to_duration(&sides[0].tracks, file_duration_seconds);
        if !split_tracks.is_empty() {
            return Some(split_tracks);
        }
        return Some(sides[0].tracks.clone());
    }
    
    // Collect all candidate track sets with their scores
    let mut candidates: Vec<(Vec<ExpectedTrack>, f64)> = Vec::new(); // (tracks, score)
    
    for side in sides {
        if side.tracks.is_empty() {
            continue;
        }
        
        // Try the whole medium
        let score = score_track_set(&side.tracks, file_duration_seconds, song_titles);
        candidates.push((side.tracks.clone(), score));
        
        // If medium duration is much larger than file, try splitting it (vinyl disc → physical sides)
        let ratio = side.total_duration / file_duration_seconds;
        if ratio > 1.3 && side.tracks.len() >= 3 {
            let (_, split_tracks) = match_tracks_to_duration(&side.tracks, file_duration_seconds);
            if !split_tracks.is_empty() {
                let score = score_track_set(&split_tracks, file_duration_seconds, song_titles);
                candidates.push((split_tracks, score));
            }
        }
    }
    
    // Pick the candidate with the highest score
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.into_iter().next().map(|(tracks, _)| tracks)
}

/// Score a set of tracks against file duration and identified song titles.
/// Higher score = better match.
/// Song title overlap is weighted heavily to prefer correct content over just duration.
fn score_track_set(tracks: &[ExpectedTrack], file_duration_seconds: f64, song_titles: &[String]) -> f64 {
    if tracks.is_empty() {
        return 0.0;
    }
    
    let total_duration: f64 = tracks.iter().map(|t| t.length_seconds).sum();
    let duration_error = (total_duration - file_duration_seconds).abs();
    let duration_ratio = duration_error / file_duration_seconds;
    
    // Duration score: 1.0 for perfect match, 0.0 for 10%+ error
    let duration_score = (1.0 - duration_ratio * 10.0).max(0.0);
    
    // Song title overlap score: fuzzy match identified songs against track titles
    let mut song_matches = 0;
    if !song_titles.is_empty() {
        let track_titles_lower: Vec<String> = tracks.iter()
            .map(|t| t.title.to_lowercase())
            .collect();
        
        for song in song_titles {
            let song_lower = song.to_lowercase();
            // Split song title into significant words (3+ chars) for fuzzy matching
            let song_words: Vec<&str> = song_lower.split_whitespace()
                .filter(|w| w.len() >= 3)
                .collect();
            
            for track_title in &track_titles_lower {
                // Check if any significant word from the song appears in the track title
                let word_matches = song_words.iter()
                    .filter(|w| track_title.contains(**w))
                    .count();
                if word_matches >= 1 && (word_matches as f64 / song_words.len().max(1) as f64) >= 0.3 {
                    song_matches += 1;
                    break;
                }
            }
        }
    }
    
    let max_songs = song_titles.len().max(1) as f64;
    let song_score = song_matches as f64 / max_songs;
    
    // Combined score: song overlap is more important than duration
    // Song match: 0-100, Duration: 0-10
    song_score * 100.0 + duration_score * 10.0
}

/// Get the best matching duration error for a release's sides vs file duration.
/// Tries whole media and split-media (disc → physical side).
fn best_duration_error(sides: &[MediumInfo], file_duration_seconds: f64) -> f64 {
    let mut best_error = f64::MAX;
    
    for side in sides {
        if side.tracks.is_empty() {
            continue;
        }
        
        // Try whole medium
        let diff = (side.total_duration - file_duration_seconds).abs();
        if diff < best_error {
            best_error = diff;
        }
        
        // Try splitting medium (vinyl disc → physical sides)
        let ratio = side.total_duration / file_duration_seconds;
        if ratio > 1.3 && side.tracks.len() >= 3 {
            let (_, split_tracks) = match_tracks_to_duration(&side.tracks, file_duration_seconds);
            if !split_tracks.is_empty() {
                let split_dur: f64 = split_tracks.iter().map(|t| t.length_seconds).sum();
                let split_diff = (split_dur - file_duration_seconds).abs();
                if split_diff < best_error {
                    best_error = split_diff;
                }
            }
        }
    }
    
    // Also check total duration across all media
    let total_duration: f64 = sides.iter().map(|s| s.total_duration).sum();
    let total_diff = (total_duration - file_duration_seconds).abs();
    if total_diff < best_error {
        best_error = total_diff;
    }
    
    best_error
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
pub fn search_release(artist: &str, release: &str, limit: u32) -> Result<Vec<SearchResult>, Box<dyn Error>> {
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
    let mut rl = RateLimiter::from_millis("MusicBrainz", 1100);

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

        // MusicBrainz rate limit
        if split < words.len() - 1 {
            rl.wait_if_needed();
        }
    }

    // Deduplicate by release_id
    let mut seen_ids = std::collections::HashSet::new();
    all_results.retain(|r| seen_ids.insert(r.release_id.clone()));

    Ok(all_results)
}

/// Rank search results by how well their total duration matches the music duration.
/// Uses per-side data from MusicBrainz, also tries splitting media for vinyl.
pub fn rank_by_duration_match(
    results: &[SearchResult],
    music_duration_seconds: f64,
    verbose: bool,
) -> Result<Vec<(SearchResult, f64)>, Box<dyn Error>> {
    let mut ranked = Vec::new();
    let mut rl = RateLimiter::from_millis("MusicBrainz", 1100);

    for result in results {
        // Fetch full track info with per-side data
        let sides = match fetch_release_sides(&result.release_id) {
            Ok(s) => s,
            Err(e) => {
                if verbose {
                    eprintln!("  Failed to fetch tracks for {}: {}", result.release_id, e);
                }
                continue;
            }
        };

        if sides.is_empty() {
            continue;
        }

        let best_error = best_duration_error(&sides, music_duration_seconds);

        if verbose {
            eprintln!("  {} - {}: {} media, best error {:.1}s",
                     result.artist, result.title, sides.len(), best_error);
        }

        ranked.push((result.clone(), best_error));

        // MusicBrainz rate limit
        rl.wait_if_needed();
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

/// Search MusicBrainz for recordings matching a song title and artist.
/// Returns releases that contain the matching recordings.
fn search_recording(artist: &str, title: &str, limit: u32) -> Result<Vec<SearchResult>, Box<dyn Error>> {
    // URL-encode the query
    let artist_q = artist.replace(' ', "+");
    let title_q = title.replace(' ', "+");

    let url = format!(
        "https://musicbrainz.org/ws/2/recording/?query=recording:{}+artist:{}&fmt=json&limit={}",
        title_q, artist_q, limit
    );

    let response = ureq::get(&url)
        .set("User-Agent", "HiFiBerryAutoRec/0.1 (https://github.com/hifiberry/autorec)")
        .call()?;

    let search: RecordingSearchResponse = serde_json::from_reader(response.into_reader())?;

    let mut results = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for recording in search.recordings {
        if recording.score < 80 {
            continue;
        }

        for release in recording.releases {
            if !seen_ids.insert(release.id.clone()) {
                continue;
            }

            let artist_name = release.artist_credit.first()
                .map(|ac| ac.name.clone())
                .unwrap_or_default();

            let is_vinyl = release.media.iter().any(|m| {
                m.format.as_deref().map_or(false, |f| f.contains("Vinyl"))
            });

            let track_count = release.media.iter()
                .filter_map(|m| m.track_count)
                .sum::<u32>();

            results.push(SearchResult {
                release_id: release.id,
                title: release.title,
                artist: artist_name,
                score: recording.score,
                is_vinyl,
                track_count,
            });
        }
    }

    Ok(results)
}

/// Find the best album for a set of identified songs by searching MusicBrainz recordings.
///
/// For each unique song (deduplicated by title+artist), searches the MusicBrainz
/// recording API to find which releases contain it. Then ranks releases by:
/// 1. Number of matching songs (more is better)
/// 2. Duration match (closer to music_duration is better)
///
/// When `vinyl_only` is true, only vinyl releases are considered.
///
/// Returns the best matching release and the number of songs that matched.
pub fn find_album_by_songs(
    songs: &[IdentifiedSong],
    music_duration_seconds: f64,
    vinyl_only: bool,
    verbose: bool,
) -> Result<Option<(SearchResult, usize)>, Box<dyn Error>> {
    if songs.is_empty() {
        return Ok(None);
    }

    // Deduplicate songs by (artist, title) - case insensitive
    let mut unique_songs: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for song in songs {
        let key = (song.artist.to_lowercase(), song.title.to_lowercase());
        if seen.insert(key) {
            unique_songs.push((song.artist.clone(), song.title.clone()));
        }
    }

    println!("Searching for {} unique song(s) on MusicBrainz...", unique_songs.len());

    // For each unique song, search MusicBrainz recordings and collect release IDs
    // release_id -> (SearchResult, match_count)
    let mut release_counts: std::collections::HashMap<String, (SearchResult, usize)> =
        std::collections::HashMap::new();
    let mut rl = RateLimiter::from_millis("MusicBrainz", 1100);

    for (i, (artist, title)) in unique_songs.iter().enumerate() {
        if verbose {
            println!("  [{}/{}] Searching: {} - {}", i + 1, unique_songs.len(), artist, title);
        }

        match search_recording(artist, title, 10) {
            Ok(releases) => {
                if verbose {
                    println!("    Found {} releases", releases.len());
                }
                for r in releases {
                    release_counts.entry(r.release_id.clone())
                        .and_modify(|(_, count)| *count += 1)
                        .or_insert((r, 1));
                }
            }
            Err(e) => {
                if verbose {
                    println!("    Search failed: {}", e);
                }
            }
        }

        // MusicBrainz rate limit
        rl.wait_if_needed();
    }

    if release_counts.is_empty() {
        println!("No releases found containing the identified songs");
        return Ok(None);
    }

    // Sort by number of matching songs (descending)
    let mut candidates: Vec<(SearchResult, usize)> = release_counts.into_values().collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    // When vinyl_only, filter to vinyl releases (but keep all if none are vinyl)
    if vinyl_only {
        let vinyl_candidates: Vec<(SearchResult, usize)> = candidates.iter()
            .filter(|(r, _)| r.is_vinyl)
            .cloned()
            .collect();
        if !vinyl_candidates.is_empty() {
            println!("Filtered to {} vinyl releases (from {} total)", vinyl_candidates.len(), candidates.len());
            candidates = vinyl_candidates;
            candidates.sort_by(|a, b| b.1.cmp(&a.1));
        } else {
            println!("No vinyl releases found, using all {} releases", candidates.len());
        }
    }

    let max_song_count = candidates[0].1;
    println!("Found {} releases, best candidates match {} song(s)", candidates.len(), max_song_count);

    // Take top candidates: those with at least (max - 1) matching songs, up to 15
    let top_candidates: Vec<(SearchResult, usize)> = candidates.into_iter()
        .filter(|(_, count)| *count >= max_song_count.saturating_sub(1))
        .take(15)
        .collect();

    if verbose {
        for (r, count) in &top_candidates {
            println!("  {} - {} ({} songs, vinyl: {})", r.artist, r.title, count, r.is_vinyl);
        }
    }

    println!("Ranking {} candidates by duration match...", top_candidates.len());

    // Rank by duration match
    let search_results: Vec<SearchResult> = top_candidates.iter().map(|(r, _)| r.clone()).collect();
    let song_counts: std::collections::HashMap<String, usize> = top_candidates.into_iter()
        .map(|(r, count)| (r.release_id, count))
        .collect();

    let ranked = rank_by_duration_match(&search_results, music_duration_seconds, verbose)?;

    if ranked.is_empty() {
        return Ok(None);
    }

    let (best, error) = &ranked[0];
    let best_song_count = song_counts.get(&best.release_id).copied().unwrap_or(0);

    // Accept if error is within 5% or 30 seconds (whichever is larger)
    let threshold = (music_duration_seconds * 0.05).max(30.0);
    if *error > threshold {
        println!("Best match duration error too large: {:.1}s (threshold: {:.1}s)", error, threshold);
        return Ok(None);
    }

    Ok(Some((best.clone(), best_song_count)))
}
