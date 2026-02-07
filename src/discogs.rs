//! Discogs API client for vinyl release identification.
//!
//! Uses the Discogs REST API to look up releases with per-side tracklists.
//! The key advantage over MusicBrainz is that Discogs track positions contain
//! explicit side letters (A1, A2, B1, B2, C1, …).
//!
//! Authentication: uses Discogs key+secret (from discogs_credentials.toml or
//! `/etc/autorec/discogs_credentials.toml`).  Without credentials the API allows
//! 25 req/min; with credentials 60 req/min.

use serde::Deserialize;
use std::error::Error;

use crate::album_identifier::IdentifiedSong;
use crate::rate_limiter::RateLimiter;

// ── Discogs credentials ──────────────────────────────────────────────────────

/// Consumer key + secret for Discogs API.
struct Credentials {
    key: String,
    secret: String,
}

/// Try to load credentials from known paths, return None if not found.
fn load_credentials() -> Option<Credentials> {
    let paths = [
        // Next to the binary / workspace root
        "discogs_credentials.toml",
        // System-wide
        "/etc/autorec/discogs_credentials.toml",
    ];

    for path in &paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(table) = content.parse::<toml::Table>() {
                let key = table.get("consumer_key")?.as_str()?.to_string();
                let secret = table.get("consumer_secret")?.as_str()?.to_string();
                return Some(Credentials { key, secret });
            }
        }
    }

    // Try home directory
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::PathBuf::from(home)
            .join(".config/autorec/discogs_credentials.toml");
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(table) = content.parse::<toml::Table>() {
                let key = table.get("consumer_key")?.as_str()?.to_string();
                let secret = table.get("consumer_secret")?.as_str()?.to_string();
                return Some(Credentials { key, secret });
            }
        }
    }

    None
}

const USER_AGENT: &str = "HifiBerryAutorec/0.2 +https://github.com/hifiberry/autorec";

/// Create a rate limiter for Discogs.
/// Authenticated: 60 req/min → 1.0 s base interval.
/// Unauthenticated: 25 req/min → 2.5 s base interval.
pub fn create_rate_limiter(authenticated: bool) -> RateLimiter {
    if authenticated {
        RateLimiter::from_millis("Discogs", 1000)
    } else {
        RateLimiter::from_millis("Discogs", 2500)
    }
}

// ── API response types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ApiRelease {
    id: u64,
    title: String,
    #[serde(default)]
    artists: Vec<ApiArtist>,
    #[serde(default)]
    tracklist: Vec<ApiTrack>,
    #[serde(default)]
    formats: Vec<ApiFormat>,
    year: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ApiArtist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiTrack {
    position: String,
    title: String,
    duration: String,
    #[serde(rename = "type_")]
    track_type: String,
}

#[derive(Debug, Deserialize)]
struct ApiFormat {
    name: String,
    #[serde(default)]
    descriptions: Vec<String>,
    #[serde(default)]
    qty: String,
}

#[derive(Debug, Deserialize)]
struct ApiMaster {
    id: u64,
    title: String,
    #[serde(default)]
    artists: Vec<ApiArtist>,
    main_release: Option<u64>,
    tracklist: Option<Vec<ApiTrack>>,
}

#[derive(Debug, Deserialize)]
struct ApiVersionsResponse {
    pagination: ApiPagination,
    versions: Vec<ApiVersion>,
}

#[derive(Debug, Deserialize)]
struct ApiPagination {
    items: u64,
    #[allow(dead_code)]
    pages: u64,
}

#[derive(Debug, Deserialize)]
struct ApiVersion {
    id: u64,
    title: String,
    format: String,
    country: Option<String>,
    released: Option<String>,
    #[serde(default)]
    major_formats: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ApiSearchResponse {
    pagination: ApiPagination,
    results: Vec<ApiSearchResult>,
}

#[derive(Debug, Deserialize)]
struct ApiSearchResult {
    id: u64,
    title: String,
    #[serde(default)]
    format: Vec<String>,
    country: Option<String>,
    year: Option<String>,
    master_id: Option<u64>,
    #[serde(rename = "type")]
    result_type: Option<String>,
}

// ── Public types ─────────────────────────────────────────────────────────────

/// A track from a Discogs release, with its original position label.
#[derive(Debug, Clone)]
pub struct DiscogsTrack {
    /// Original position string, e.g. "A1", "B2.a", "C3"
    pub position: String,
    /// Side letter derived from position, e.g. 'A', 'B', 'C'
    pub side: char,
    pub title: String,
    /// Duration in seconds (0.0 if not available)
    pub duration_secs: f64,
}

/// A physical side of a vinyl release (e.g. all tracks starting with "A").
#[derive(Debug, Clone)]
pub struct DiscogsSide {
    pub label: char,
    pub tracks: Vec<DiscogsTrack>,
    pub total_duration: f64,
}

/// A Discogs release with structured per-side data.
#[derive(Debug, Clone)]
pub struct DiscogsRelease {
    pub release_id: u64,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub is_vinyl: bool,
    pub sides: Vec<DiscogsSide>,
}

/// A search result (lightweight, before fetching full release).
#[derive(Debug, Clone)]
pub struct DiscogsSearchResult {
    pub release_id: u64,
    pub title: String,
    pub format: Vec<String>,
    pub country: Option<String>,
    pub year: Option<String>,
    pub master_id: Option<u64>,
    pub is_vinyl: bool,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a Discogs duration string like "6:40" or "1:02:30" into seconds.
fn parse_duration(s: &str) -> f64 {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let mins: f64 = parts[0].parse().unwrap_or(0.0);
            let secs: f64 = parts[1].parse().unwrap_or(0.0);
            mins * 60.0 + secs
        }
        3 => {
            let hrs: f64 = parts[0].parse().unwrap_or(0.0);
            let mins: f64 = parts[1].parse().unwrap_or(0.0);
            let secs: f64 = parts[2].parse().unwrap_or(0.0);
            hrs * 3600.0 + mins * 60.0 + secs
        }
        _ => 0.0,
    }
}

/// Extract the side letter from a position string.
/// "A1" → 'A', "B2.a" → 'B', "C3" → 'C', "" → '?'
fn side_from_position(pos: &str) -> char {
    pos.chars().next()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('?')
}

/// Parse a Discogs release URL like:
/// `https://www.discogs.com/release/30298511-DJ-Shadow-Endtroducing`
/// Returns the numeric release ID.
pub fn parse_discogs_url(url: &str) -> Option<u64> {
    // Try /release/12345 pattern
    if let Some(idx) = url.find("/release/") {
        let after = &url[idx + 9..];
        // Take digits until non-digit
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        return digits.parse().ok();
    }
    // Maybe it's just a number
    url.trim().parse().ok()
}

// ── API functions ────────────────────────────────────────────────────────────

/// Build a ureq request with proper auth and user-agent headers.
fn api_get(url: &str) -> ureq::Request {
    let req = ureq::get(url).set("User-Agent", USER_AGENT);

    if let Some(creds) = load_credentials() {
        req.set("Authorization",
                &format!("Discogs key={}, secret={}", creds.key, creds.secret))
    } else {
        req
    }
}

/// Check if we have credentials (determines rate limit).
pub fn has_credentials() -> bool {
    load_credentials().is_some()
}

/// Fetch a single release by ID and parse into structured sides.
pub fn fetch_release(release_id: u64, rate_limiter: &mut RateLimiter) -> Result<DiscogsRelease, Box<dyn Error>> {
    let url = format!("https://api.discogs.com/releases/{}", release_id);

    rate_limiter.wait_if_needed();

    let response = api_get(&url).call()?;
    let api: ApiRelease = serde_json::from_reader(response.into_reader())?;

    rate_limiter.report_success();

    let artist = api.artists.first()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let is_vinyl = api.formats.iter().any(|f|
        f.name.eq_ignore_ascii_case("Vinyl") ||
        f.descriptions.iter().any(|d| d.contains("LP"))
    );

    // Parse tracks and group by side
    let tracks: Vec<DiscogsTrack> = api.tracklist.iter()
        .filter(|t| t.track_type == "track")
        .map(|t| {
            let dur = parse_duration(&t.duration);
            DiscogsTrack {
                position: t.position.clone(),
                side: side_from_position(&t.position),
                title: t.title.clone(),
                duration_secs: dur,
            }
        })
        .collect();

    let sides = group_into_sides(&tracks);

    Ok(DiscogsRelease {
        release_id: api.id,
        title: api.title,
        artist,
        year: api.year,
        is_vinyl,
        sides,
    })
}

/// Group a flat track list into sides by their side letter.
fn group_into_sides(tracks: &[DiscogsTrack]) -> Vec<DiscogsSide> {
    let mut side_map: std::collections::BTreeMap<char, Vec<DiscogsTrack>> = std::collections::BTreeMap::new();

    for track in tracks {
        side_map.entry(track.side)
            .or_default()
            .push(track.clone());
    }

    side_map.into_iter()
        .map(|(label, tracks)| {
            let total_duration = tracks.iter().map(|t| t.duration_secs).sum();
            DiscogsSide { label, tracks, total_duration }
        })
        .collect()
}

/// Fetch the master release to get its ID and main release.
pub fn fetch_master(master_id: u64, rate_limiter: &mut RateLimiter) -> Result<(String, String, Option<u64>), Box<dyn Error>> {
    let url = format!("https://api.discogs.com/masters/{}", master_id);

    rate_limiter.wait_if_needed();

    let response = api_get(&url).call()?;
    let api: ApiMaster = serde_json::from_reader(response.into_reader())?;

    rate_limiter.report_success();

    let artist = api.artists.first()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    Ok((api.title, artist, api.main_release))
}

/// Fetch vinyl versions of a master release.
pub fn fetch_master_vinyl_versions(
    master_id: u64,
    rate_limiter: &mut RateLimiter,
) -> Result<Vec<DiscogsSearchResult>, Box<dyn Error>> {
    let url = format!(
        "https://api.discogs.com/masters/{}/versions?format=Vinyl&per_page=50",
        master_id
    );

    rate_limiter.wait_if_needed();

    let response = api_get(&url).call()?;
    let api: ApiVersionsResponse = serde_json::from_reader(response.into_reader())?;

    rate_limiter.report_success();

    let results = api.versions.into_iter()
        .map(|v| {
            let is_vinyl = v.major_formats.iter().any(|f| f == "Vinyl");
            DiscogsSearchResult {
                release_id: v.id,
                title: v.title,
                format: v.format.split(", ").map(|s| s.to_string()).collect(),
                country: v.country,
                year: v.released,
                master_id: Some(master_id),
                is_vinyl,
            }
        })
        .collect();

    Ok(results)
}

/// Search Discogs for releases.  **Requires authentication** (key+secret).
/// Returns an error if no credentials are available.
pub fn search_releases(
    query: &str,
    release_type: Option<&str>,
    format: Option<&str>,
    rate_limiter: &mut RateLimiter,
) -> Result<Vec<DiscogsSearchResult>, Box<dyn Error>> {
    if !has_credentials() {
        return Err("Discogs search requires authentication — missing discogs_credentials.toml".into());
    }

    let mut url = format!(
        "https://api.discogs.com/database/search?q={}&per_page=25",
        urlencoded(query)
    );

    if let Some(t) = release_type {
        url.push_str(&format!("&type={}", t));
    }
    if let Some(f) = format {
        url.push_str(&format!("&format={}", f));
    }

    rate_limiter.wait_if_needed();

    let response = api_get(&url).call()?;
    let api: ApiSearchResponse = serde_json::from_reader(response.into_reader())?;

    rate_limiter.report_success();

    let results = api.results.into_iter()
        .map(|r| {
            let is_vinyl = r.format.iter().any(|f|
                f.contains("Vinyl") || f.contains("LP")
            );
            DiscogsSearchResult {
                release_id: r.id,
                title: r.title,
                format: r.format,
                country: r.country,
                year: r.year,
                master_id: r.master_id,
                is_vinyl,
            }
        })
        .collect();

    Ok(results)
}

/// Minimal URL-encoding (spaces and a few special characters).
fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
     .replace('&', "%26")
     .replace('=', "%3D")
     .replace('#', "%23")
}

// ── Side matching ────────────────────────────────────────────────────────────

/// Find the best matching side for a given file duration and identified songs.
/// Uses both duration match and song title overlap (same scoring as musicbrainz).
pub fn find_best_side<'a>(
    release: &'a DiscogsRelease,
    file_duration_seconds: f64,
    song_titles: &[String],
    verbose: bool,
) -> Option<&'a DiscogsSide> {
    if release.sides.is_empty() {
        return None;
    }

    let mut best_side = None;
    let mut best_score = f64::NEG_INFINITY;

    for side in &release.sides {
        if side.tracks.is_empty() {
            continue;
        }

        let score = score_side(side, file_duration_seconds, song_titles);

        if verbose {
            println!("  Side {}: {:.1}s, {} tracks, score={:.1}",
                     side.label, side.total_duration, side.tracks.len(), score);
            for t in &side.tracks {
                println!("    {} {} ({:.0}s)", t.position, t.title, t.duration_secs);
            }
        }

        if score > best_score {
            best_score = score;
            best_side = Some(side);
        }
    }

    best_side
}

/// Score a side against file duration and identified song titles.
fn score_side(side: &DiscogsSide, file_duration_seconds: f64, song_titles: &[String]) -> f64 {
    let duration_error = (side.total_duration - file_duration_seconds).abs();
    let duration_ratio = duration_error / file_duration_seconds;

    // Duration score: 1.0 for perfect, 0.0 for ≥10 % error
    let duration_score = (1.0 - duration_ratio * 10.0).max(0.0);

    // Song title overlap
    let mut song_matches = 0;
    if !song_titles.is_empty() {
        let track_titles_lower: Vec<String> = side.tracks.iter()
            .map(|t| t.title.to_lowercase())
            .collect();

        for song in song_titles {
            let song_lower = song.to_lowercase();
            let song_words: Vec<&str> = song_lower.split_whitespace()
                .filter(|w| w.len() >= 3)
                .collect();

            for track_title in &track_titles_lower {
                let word_matches = song_words.iter()
                    .filter(|w| track_title.contains(**w))
                    .count();
                if word_matches >= 1
                    && (word_matches as f64 / song_words.len().max(1) as f64) >= 0.3
                {
                    song_matches += 1;
                    break;
                }
            }
        }
    }

    let max_songs = song_titles.len().max(1) as f64;
    let song_score = song_matches as f64 / max_songs;

    // Combined: song overlap is more important
    song_score * 100.0 + duration_score * 10.0
}

/// Convert a Discogs side's tracks into the MusicBrainz `ExpectedTrack` format
/// so that existing CUE generation code can use them directly.
pub fn side_to_expected_tracks(side: &DiscogsSide) -> Vec<crate::musicbrainz::ExpectedTrack> {
    let mut cumulative = 0.0;
    side.tracks.iter()
        .map(|t| {
            let et = crate::musicbrainz::ExpectedTrack {
                position: t.position.chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0),
                title: t.title.clone(),
                length_seconds: t.duration_secs,
                expected_start: cumulative,
            };
            cumulative += t.duration_secs;
            et
        })
        .collect()
}

/// High-level: given identified songs, search Discogs for the album,
/// fetch the best vinyl release, and return the matching side's tracks.
///
/// Flow:
/// 1. Determine artist + album from the identified songs
/// 2. Search Discogs for the master release
/// 3. Get vinyl versions of the master, preferring recent pressings
/// 4. Fetch top candidates and pick the one whose best side matches
///    both the file duration and the identified song titles
pub fn find_album_by_songs(
    songs: &[IdentifiedSong],
    file_duration_seconds: f64,
    vinyl_only: bool,
    verbose: bool,
) -> Result<Option<DiscogsRelease>, Box<dyn Error>> {
    if songs.is_empty() {
        return Ok(None);
    }

    if !has_credentials() {
        if verbose {
            println!("Discogs: no credentials, skipping");
        }
        return Ok(None);
    }

    let mut rl = create_rate_limiter(true);

    // Determine the most common artist and album from identified songs
    let (artist, album) = most_common_artist_album(songs);

    if verbose {
        println!("Discogs search: artist=\"{}\" album=\"{}\"", artist, album);
    }

    let query = if album.is_empty() || album == "Unknown" {
        artist.clone()
    } else {
        format!("{} {}", artist, album)
    };

    // ── Step 1: find the master release ──────────────────────────────────
    let master_id = {
        let results = search_releases(&query, Some("master"), None, &mut rl)?;
        if verbose {
            println!("Master search: {} results", results.len());
            for r in results.iter().take(3) {
                println!("  id={} \"{}\" master={:?}", r.release_id, r.title, r.master_id);
            }
        }
        match results.first() {
            Some(r) => r.master_id.unwrap_or(r.release_id),
            None => {
                // Fallback: try direct release search
                if verbose { println!("No master found, trying direct release search"); }
                let format_filter = if vinyl_only { Some("Vinyl") } else { None };
                let results = search_releases(&query, Some("release"), format_filter, &mut rl)?;
                if results.is_empty() {
                    if verbose { println!("No Discogs results found"); }
                    return Ok(None);
                }
                // Fetch a few directly and pick the best
                return pick_best_from_search(&results, songs, file_duration_seconds, vinyl_only, verbose, &mut rl);
            }
        }
    };

    if verbose {
        println!("Using master ID: {}", master_id);
    }

    // ── Step 2: get vinyl versions of the master ─────────────────────────
    let versions = fetch_master_vinyl_versions(master_id, &mut rl)?;

    if versions.is_empty() {
        if verbose { println!("No vinyl versions found for master {}", master_id); }
        return Ok(None);
    }

    if verbose {
        println!("Found {} vinyl versions", versions.len());
    }

    // Sort versions: prefer recent pressings (likely to match user's copy)
    let mut sorted_versions = versions;
    sorted_versions.sort_by(|a, b| {
        let ya = a.year.as_deref().and_then(|y| y.parse::<u32>().ok()).unwrap_or(0);
        let yb = b.year.as_deref().and_then(|y| y.parse::<u32>().ok()).unwrap_or(0);
        yb.cmp(&ya) // newest first
    });

    if verbose {
        for (i, v) in sorted_versions.iter().take(5).enumerate() {
            println!("  {}. id={} \"{}\" year={:?}", i + 1, v.release_id, v.title, v.year);
        }
    }

    // ── Step 3: fetch top candidates and score ───────────────────────────
    let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();
    let mut best_release: Option<DiscogsRelease> = None;
    let mut best_score = f64::NEG_INFINITY;

    // Fetch up to 8 releases (newest first), stop early if we find a great match
    for v in sorted_versions.iter().take(8) {
        let release = match fetch_release(v.release_id, &mut rl) {
            Ok(r) => r,
            Err(e) => {
                if verbose {
                    println!("  Failed to fetch release {}: {}", v.release_id, e);
                }
                continue;
            }
        };

        if let Some(side) = find_best_side(&release, file_duration_seconds, &song_titles, false) {
            let score = score_side(side, file_duration_seconds, &song_titles);

            if verbose {
                println!("  Release {} ({}) — best side {}: score={:.1} ({:.0}s, {} tracks)",
                         release.release_id,
                         release.year.map_or("?".into(), |y: u32| y.to_string()),
                         side.label, score, side.total_duration, side.tracks.len());
            }

            if score > best_score {
                best_score = score;
                best_release = Some(release);
            }

            // Perfect song match + good duration → stop early
            if score >= 100.0 {
                if verbose { println!("  → Perfect match, stopping search"); }
                break;
            }
        }
    }

    if verbose {
        if let Some(ref r) = best_release {
            println!("Selected: {} - {} (id={}, score={:.1})",
                     r.artist, r.title, r.release_id, best_score);
        }
    }

    Ok(best_release)
}

/// Helper: pick best release directly from search results.
fn pick_best_from_search(
    results: &[DiscogsSearchResult],
    songs: &[IdentifiedSong],
    file_duration_seconds: f64,
    vinyl_only: bool,
    verbose: bool,
    rl: &mut RateLimiter,
) -> Result<Option<DiscogsRelease>, Box<dyn Error>> {
    let mut candidates: Vec<&DiscogsSearchResult> = results.iter().collect();
    if vinyl_only {
        let vinyl: Vec<&DiscogsSearchResult> = candidates.iter()
            .filter(|r| r.is_vinyl).copied().collect();
        if !vinyl.is_empty() { candidates = vinyl; }
    }

    let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();
    let mut best_release: Option<DiscogsRelease> = None;
    let mut best_score = f64::NEG_INFINITY;

    for c in candidates.iter().take(5) {
        let release = match fetch_release(c.release_id, rl) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(side) = find_best_side(&release, file_duration_seconds, &song_titles, verbose) {
            let score = score_side(side, file_duration_seconds, &song_titles);
            if score > best_score {
                best_score = score;
                best_release = Some(release);
            }
        }
    }
    Ok(best_release)
}

/// Determine the most common artist and album from a list of identified songs.
fn most_common_artist_album(songs: &[IdentifiedSong]) -> (String, String) {
    use std::collections::HashMap;

    let mut artist_counts: HashMap<String, usize> = HashMap::new();
    let mut album_counts: HashMap<String, usize> = HashMap::new();

    for song in songs {
        *artist_counts.entry(song.artist.clone()).or_default() += 1;
        if let Some(ref album) = song.album {
            *album_counts.entry(album.clone()).or_default() += 1;
        }
    }

    let artist = artist_counts.into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(a, _)| a)
        .unwrap_or_else(|| "Unknown".to_string());

    let album = album_counts.into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(a, _)| a)
        .unwrap_or_else(|| "Unknown".to_string());

    (artist, album)
}
