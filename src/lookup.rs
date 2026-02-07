//! Album/side identification with pluggable backends and fallback strategy.
//!
//! The [`AlbumSideIdentifier`] trait defines a common interface for looking up
//! which album and side a set of identified songs belong to.  Two backends are
//! provided:
//!
//! * [`DiscogsBackend`] – uses the Discogs API (explicit side letters A/B/C/D)
//! * [`MusicBrainzBackend`] – uses the MusicBrainz API (with optional vinyl filter)
//!
//! [`find_album_side_with_fallback`] tries each backend in order and returns the
//! first successful result.

use std::error::Error;

use crate::album_identifier::IdentifiedSong;
use crate::musicbrainz;

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

// ── Trait ─────────────────────────────────────────────────────────────────────

/// A backend that can identify which album and side a set of songs belong to.
pub trait AlbumSideIdentifier {
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

// ── Discogs backend ──────────────────────────────────────────────────────────

/// Looks up the album via the Discogs API.
/// Discogs track positions carry explicit side letters (A1, B2, C3, …).
pub struct DiscogsBackend;

impl AlbumSideIdentifier for DiscogsBackend {
    fn name(&self) -> &str {
        "Discogs"
    }

    fn find_album_side(
        &self,
        songs: &[IdentifiedSong],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<AlbumSideResult>, Box<dyn Error>> {
        use crate::discogs;

        let release = match discogs::find_album_by_songs(
            songs,
            file_duration_seconds,
            true, // vinyl_only
            verbose,
        )? {
            Some(r) => r,
            None => return Ok(None),
        };

        let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();

        let side = match discogs::find_best_side(
            &release,
            file_duration_seconds,
            &song_titles,
            verbose,
        ) {
            Some(s) => s,
            None => return Ok(None),
        };

        let tracks = discogs::side_to_expected_tracks(side);
        if tracks.is_empty() {
            return Ok(None);
        }

        Ok(Some(AlbumSideResult {
            artist: release.artist,
            album_title: release.title,
            release_info: format!(
                "https://www.discogs.com/release/{}",
                release.release_id
            ),
            tracks,
            backend: "Discogs".to_string(),
        }))
    }
}

// ── MusicBrainz backend ──────────────────────────────────────────────────────

/// Looks up the album via the MusicBrainz API.
/// When `vinyl_only` is true only vinyl releases are considered.
pub struct MusicBrainzBackend {
    pub vinyl_only: bool,
}

impl AlbumSideIdentifier for MusicBrainzBackend {
    fn name(&self) -> &str {
        if self.vinyl_only {
            "MusicBrainz (vinyl)"
        } else {
            "MusicBrainz (all)"
        }
    }

    fn find_album_side(
        &self,
        songs: &[IdentifiedSong],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<AlbumSideResult>, Box<dyn Error>> {
        let (best, _song_count) = match musicbrainz::find_album_by_songs(
            songs,
            file_duration_seconds,
            self.vinyl_only,
            verbose,
        )? {
            Some(r) => r,
            None => return Ok(None),
        };

        let sides = musicbrainz::fetch_release_sides(&best.release_id)?;

        let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();

        let side_tracks =
            if let Some(tracks) = musicbrainz::find_best_side(&sides, file_duration_seconds, &song_titles) {
                tracks
            } else {
                // Fallback: flatten all tracks and split by duration
                let all_tracks: Vec<musicbrainz::ExpectedTrack> =
                    sides.iter().flat_map(|s| s.tracks.clone()).collect();
                let (_, t) =
                    musicbrainz::match_tracks_to_duration(&all_tracks, file_duration_seconds);
                t
            };

        if side_tracks.is_empty() {
            return Ok(None);
        }

        Ok(Some(AlbumSideResult {
            artist: best.artist,
            album_title: best.title,
            release_info: format!(
                "https://musicbrainz.org/release/{}",
                best.release_id
            ),
            tracks: side_tracks,
            backend: self.name().to_string(),
        }))
    }

    fn fetch_durations_for_album(
        &self,
        artist: &str,
        album_title: &str,
        track_titles: &[String],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<Vec<musicbrainz::ExpectedTrack>>, Box<dyn Error>> {
        self.search_album_for_durations(artist, album_title, track_titles, file_duration_seconds, verbose)
    }
}

impl MusicBrainzBackend {
    /// Search MusicBrainz for a release by artist+album, return the best
    /// matching side's tracks (with duration data).
    fn search_album_for_durations(
        &self,
        artist: &str,
        album_title: &str,
        track_titles: &[String],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<Vec<musicbrainz::ExpectedTrack>>, Box<dyn Error>> {
        use crate::rate_limiter::RateLimiter;

        let mut rl = RateLimiter::from_millis("MusicBrainz", 1100);

        let results = musicbrainz::search_release(artist, album_title, 10)?;
        rl.wait_if_needed();

        if results.is_empty() {
            if verbose {
                println!("  [{}] No releases found for duration enrichment", self.name());
            }
            return Ok(None);
        }

        // Optionally filter to vinyl
        let candidates: Vec<_> = if self.vinyl_only {
            let vinyl: Vec<_> = results.iter().filter(|r| r.is_vinyl).cloned().collect();
            if vinyl.is_empty() { results } else { vinyl }
        } else {
            results
        };

        for result in &candidates {
            let sides = match musicbrainz::fetch_release_sides(&result.release_id) {
                Ok(s) => s,
                Err(_) => { rl.wait_if_needed(); continue; }
            };
            rl.wait_if_needed();

            if let Some(tracks) = musicbrainz::find_best_side(&sides, file_duration_seconds, track_titles) {
                let total_dur: f64 = tracks.iter().map(|t| t.length_seconds).sum();
                if total_dur > 0.0 {
                    if verbose {
                        println!("  [{}] Found durations from release {}",
                                 self.name(), result.release_id);
                    }
                    return Ok(Some(tracks));
                }
            }
        }

        Ok(None)
    }
}

// ── Fallback strategy ────────────────────────────────────────────────────────

/// Try each backend in order.  Returns the first successful result.
///
/// When the winning backend returns tracks without duration data (all 0 s),
/// the remaining backends are asked to enrich the result via
/// [`AlbumSideIdentifier::fetch_durations_for_album`].
pub fn find_album_side_with_fallback(
    backends: &[&dyn AlbumSideIdentifier],
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
