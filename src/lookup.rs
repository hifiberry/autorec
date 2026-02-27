//! Album/side identification with pluggable backends and fallback strategy.
//!
//! The [`AlbumIdentifier`] trait defines a common interface for looking up
//! which album and side a set of identified songs belong to.  Implementations
//! live in separate modules:
//!
//! * [`lookup_discogs::DiscogsBackend`]
//! * [`lookup_musicbrainz::MusicBrainzBackend`]
//!
//! [`find_album_side_with_fallback`] tries each backend in order and returns the
//! first successful result.

use std::error::Error;

use crate::album_identifier::IdentifiedSong;
use crate::musicbrainz;

// Re-export backends so existing `use autorec::lookup::{DiscogsBackend, …}` keeps working.
pub use crate::lookup_discogs::DiscogsBackend;
pub use crate::lookup_musicbrainz::MusicBrainzBackend;

// ── Common result type ───────────────────────────────────────────────────────

/// Unified result from any album/side identification backend.
#[derive(Debug, Clone)]
pub struct AlbumSideResult {
    /// Artist name
    pub artist: String,
    /// Album / release title
    pub album_title: String,
    /// Human-readable release reference (URL or ID string)
    pub release_info: String,
    /// Ordered track list for the matched side (in `ExpectedTrack` format)
    pub tracks: Vec<musicbrainz::ExpectedTrack>,
    /// Name of the backend that produced this result
    pub backend: String,
}

impl AlbumSideResult {
    /// Check whether the tracks carry usable (non-zero) duration data.
    /// Returns `false` when every track has a 0s length (common on Discogs).
    pub fn has_usable_durations(&self) -> bool {
        self.tracks.iter().any(|t| t.length_seconds > 0.0)
    }
}

/// One side of a vinyl release (e.g. Side A, Side B, …).
#[derive(Debug, Clone)]
pub struct SideInfo {
    /// Side letter: 'A', 'B', 'C', 'D', …
    pub label: char,
    /// Ordered track list for this side
    pub tracks: Vec<musicbrainz::ExpectedTrack>,
    /// Total duration of this side in seconds (0 when unknown)
    pub total_duration: f64,
}

/// Full album result with all sides — returned by [`AlbumIdentifier::find_album`].
#[derive(Debug, Clone)]
pub struct AlbumResult {
    /// Artist name
    pub artist: String,
    /// Album / release title
    pub album_title: String,
    /// Human-readable release reference (URL or ID string)
    pub release_info: String,
    /// All sides of the release, in order
    pub sides: Vec<SideInfo>,
    /// Name of the backend that produced this result
    pub backend: String,
}

impl AlbumResult {
    /// Check whether at least one side carries usable (non-zero) duration data.
    pub fn has_usable_durations(&self) -> bool {
        self.sides.iter().any(|s|
            s.tracks.iter().any(|t| t.length_seconds > 0.0)
        )
    }
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// A backend that can identify which album and side a set of songs belong to.
pub trait AlbumIdentifier {
    /// Short display name, e.g. "Discogs" or "MusicBrainz (vinyl)".
    fn name(&self) -> &str;

    /// Try to find the album side matching the given songs and file duration.
    /// Returns `Ok(None)` when the backend has no match (not an error).
    fn find_album_side(
        &self,
        songs: &[IdentifiedSong],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<AlbumSideResult>, Box<dyn Error>>;

    /// Try to find the album matching the given songs, returning **all** sides.
    /// This is used by multi-file identification where we need to assign
    /// different files to different sides of the same album.
    ///
    /// The default implementation calls `find_album_side()` and returns a
    /// single-side album.  Backends that have full per-side data should
    /// override this for proper multi-side results.
    fn find_album(
        &self,
        songs: &[IdentifiedSong],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<AlbumResult>, Box<dyn Error>> {
        // Default: fall back to find_album_side and wrap in an AlbumResult
        let side = match self.find_album_side(songs, file_duration_seconds, verbose)? {
            Some(s) => s,
            None => return Ok(None),
        };
        let total_dur: f64 = side.tracks.iter().map(|t| t.length_seconds).sum();
        Ok(Some(AlbumResult {
            artist: side.artist,
            album_title: side.album_title,
            release_info: side.release_info,
            sides: vec![SideInfo {
                label: '?',
                tracks: side.tracks,
                total_duration: total_dur,
            }],
            backend: side.backend,
        }))
    }

    /// Given an album already identified by another backend, try to fetch
    /// tracks with duration data for the matching side.
    ///
    /// This is called when the identifying backend returned tracks without
    /// duration information (e.g. Discogs releases with 0 s durations).
    /// The default implementation returns `Ok(None)` (no enrichment available).
    fn fetch_durations_for_album(
        &self,
        _artist: &str,
        _album_title: &str,
        _track_titles: &[String],
        _file_duration_seconds: f64,
        _verbose: bool,
    ) -> Result<Option<Vec<musicbrainz::ExpectedTrack>>, Box<dyn Error>> {
        Ok(None)
    }
}

// ── Fallback strategy ────────────────────────────────────────────────────────

/// Try each backend in order.  Returns the first successful result.
///
/// When the winning backend returns tracks without duration data (all 0 s),
/// the remaining backends are asked to enrich the result via
/// [`AlbumIdentifier::fetch_durations_for_album`].
pub fn find_album_side_with_fallback(
    backends: &[&dyn AlbumIdentifier],
    songs: &[IdentifiedSong],
    file_duration_seconds: f64,
    verbose: bool,
) -> Result<Option<AlbumSideResult>, Box<dyn Error>> {
    for (idx, backend) in backends.iter().enumerate() {
        println!("Trying {}...", backend.name());

        match backend.find_album_side(songs, file_duration_seconds, verbose) {
            Ok(Some(mut result)) => {
                println!(
                    "{}: found {} - {} ({} tracks)",
                    result.backend,
                    result.artist,
                    result.album_title,
                    result.tracks.len()
                );

                // If the identifying backend has no duration data, ask
                // other backends to supply durations for the same album.
                if !result.has_usable_durations() {
                    let track_titles: Vec<String> = result.tracks.iter()
                        .map(|t| t.title.clone())
                        .collect();

                    for (j, other) in backends.iter().enumerate() {
                        if j == idx { continue; }

                        println!("  Trying {} for track durations...", other.name());

                        match other.fetch_durations_for_album(
                            &result.artist,
                            &result.album_title,
                            &track_titles,
                            file_duration_seconds,
                            verbose,
                        ) {
                            Ok(Some(enriched)) => {
                                println!("  Track durations provided by {}", other.name());
                                result.tracks = enriched;
                                result.backend = format!(
                                    "{} + {} (durations)",
                                    result.backend, other.name()
                                );
                                break;
                            }
                            Ok(None) => {
                                if verbose {
                                    println!("  {}: no duration data available", other.name());
                                }
                            }
                            Err(e) => {
                                if verbose {
                                    println!("  {}: duration fetch error: {}", other.name(), e);
                                }
                            }
                        }
                    }
                }

                return Ok(Some(result));
            }
            Ok(None) => {
                println!("{}: no match found", backend.name());
            }
            Err(e) => {
                println!("{}: error: {}", backend.name(), e);
            }
        }
    }

    Ok(None)
}

/// Try each backend in order to find the full album (all sides).
/// Returns the first successful result.
pub fn find_album_with_fallback(
    backends: &[&dyn AlbumIdentifier],
    songs: &[IdentifiedSong],
    file_duration_seconds: f64,
    verbose: bool,
) -> Result<Option<AlbumResult>, Box<dyn Error>> {
    for backend in backends.iter() {
        println!("Trying {}...", backend.name());

        match backend.find_album(songs, file_duration_seconds, verbose) {
            Ok(Some(result)) => {
                println!(
                    "{}: found {} - {} ({} side(s))",
                    result.backend,
                    result.artist,
                    result.album_title,
                    result.sides.len()
                );
                return Ok(Some(result));
            }
            Ok(None) => {
                println!("{}: no match found", backend.name());
            }
            Err(e) => {
                println!("{}: error: {}", backend.name(), e);
            }
        }
    }

    Ok(None)
}

// ── Multi-file side assignment ───────────────────────────────────────────────

/// Per-file data needed for side assignment.
pub struct FileForAssignment {
    /// File path (for display and result keying)
    pub path: String,
    /// Song titles identified by Shazam for this file
    pub song_titles: Vec<String>,
    /// Duration of the file/music region in seconds
    pub duration: f64,
}

/// Per-file result after album identification and side assignment.
#[derive(Debug, Clone)]
pub struct FileSideResult {
    /// Original file path
    pub path: String,
    /// Artist name
    pub artist: String,
    /// Album title
    pub album_title: String,
    /// Human-readable release reference (URL or ID string)
    pub release_info: String,
    /// Side letter assigned ('A', 'B', … or '?' if unmatched)
    pub side_label: char,
    /// Ordered track list for the assigned side
    pub tracks: Vec<musicbrainz::ExpectedTrack>,
    /// Name of the backend that found the album
    pub backend: String,
    /// Assignment score (higher = better match; 0.0 if unmatched)
    pub score: f64,
}

/// Score how well a file's songs match an album side.
///
/// Uses song-title word overlap (weighted ×100) plus duration match
/// (weighted ×10).  Higher = better match.
pub fn score_file_vs_side(song_titles: &[String], side: &SideInfo, file_duration: f64) -> f64 {
    if side.tracks.is_empty() || song_titles.is_empty() {
        return 0.0;
    }

    let track_titles_lower: Vec<String> = side.tracks.iter()
        .map(|t| t.title.to_lowercase())
        .collect();

    let mut matches = 0;
    for song in song_titles {
        let song_lower = song.to_lowercase();
        let words: Vec<&str> = song_lower.split_whitespace()
            .filter(|w| w.len() >= 3)
            .collect();
        for tt in &track_titles_lower {
            let wm = words.iter().filter(|w| tt.contains(**w)).count();
            if wm >= 1 && (wm as f64 / words.len().max(1) as f64) >= 0.3 {
                matches += 1;
                break;
            }
        }
    }

    let song_score = matches as f64 / song_titles.len().max(1) as f64;

    let dur_score = if side.total_duration > 0.0 {
        let ratio = (side.total_duration - file_duration).abs() / file_duration;
        (1.0 - ratio * 10.0).max(0.0)
    } else {
        0.5
    };

    song_score * 100.0 + dur_score * 10.0
}

/// Assign files to album sides using a greedy algorithm.
///
/// Returns one [`FileSideResult`] per input file (in the same order).
/// Files that couldn't be matched get `side_label = '?'` and an empty track list.
pub fn assign_files_to_album_sides(
    files: &[FileForAssignment],
    album: &AlbumResult,
    verbose: bool,
) -> Vec<FileSideResult> {
    let n_files = files.len();
    let n_sides = album.sides.len();

    // Build score matrix
    let mut scores = vec![vec![0.0f64; n_sides]; n_files];
    for (fi, file) in files.iter().enumerate() {
        for (si, side) in album.sides.iter().enumerate() {
            scores[fi][si] = score_file_vs_side(&file.song_titles, side, file.duration);
        }
    }

    if verbose {
        println!("Score matrix:");
        print!("  {:>42}", "");
        for side in &album.sides {
            print!("  Side {} ", side.label);
        }
        println!();
        for (fi, file) in files.iter().enumerate() {
            let name = std::path::Path::new(&file.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&file.path);
            let s = if name.len() > 42 { &name[..42] } else { name };
            print!("  {:>42}", s);
            for si in 0..n_sides {
                print!("  {:>6.1}", scores[fi][si]);
            }
            println!();
        }
        println!();
    }

    // Greedy assignment: pick highest score, mark both file and side as used
    let mut assigned_files = std::collections::HashSet::new();
    let mut assigned_sides = std::collections::HashSet::new();
    let mut assignments: Vec<(usize, usize, f64)> = Vec::new();

    let pairs = n_files.min(n_sides);
    for _ in 0..pairs {
        let mut best = (0usize, 0usize, f64::NEG_INFINITY);
        for fi in 0..n_files {
            if assigned_files.contains(&fi) { continue; }
            for si in 0..n_sides {
                if assigned_sides.contains(&si) { continue; }
                if scores[fi][si] > best.2 {
                    best = (fi, si, scores[fi][si]);
                }
            }
        }
        if best.2 <= 0.0 { break; }
        assigned_files.insert(best.0);
        assigned_sides.insert(best.1);
        assignments.push(best);
    }

    // Build one FileSideResult per input file
    let mut result_map: std::collections::HashMap<usize, (usize, f64)> = std::collections::HashMap::new();
    for &(fi, si, sc) in &assignments {
        result_map.insert(fi, (si, sc));
    }

    files.iter().enumerate().map(|(fi, file)| {
        if let Some(&(si, score)) = result_map.get(&fi) {
            let side = &album.sides[si];
            FileSideResult {
                path: file.path.clone(),
                artist: album.artist.clone(),
                album_title: album.album_title.clone(),
                release_info: album.release_info.clone(),
                side_label: side.label,
                tracks: side.tracks.clone(),
                backend: album.backend.clone(),
                score,
            }
        } else {
            FileSideResult {
                path: file.path.clone(),
                artist: album.artist.clone(),
                album_title: album.album_title.clone(),
                release_info: album.release_info.clone(),
                side_label: '?',
                tracks: Vec::new(),
                backend: format!("{} (no side matched)", album.backend),
                score: 0.0,
            }
        }
    }).collect()
}
