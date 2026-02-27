//! MusicBrainz implementation of the [`AlbumIdentifier`] trait.

use std::error::Error;

use crate::album_identifier::IdentifiedSong;
use crate::lookup::{AlbumIdentifier, AlbumSideResult};
use crate::musicbrainz;
use crate::rate_limiter::RateLimiter;

/// Looks up the album via the MusicBrainz API.
/// When `vinyl_only` is true only vinyl releases are considered.
pub struct MusicBrainzBackend {
    pub vinyl_only: bool,
}

impl AlbumIdentifier for MusicBrainzBackend {
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
