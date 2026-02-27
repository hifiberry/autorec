//! Discogs implementation of the [`AlbumIdentifier`] trait.

use std::error::Error;

use crate::album_identifier::IdentifiedSong;
use crate::discogs;
use crate::lookup::{AlbumIdentifier, AlbumResult, AlbumSideResult, SideInfo};

/// Looks up the album via the Discogs API.
/// Discogs track positions carry explicit side letters (A1, B2, C3, …).
pub struct DiscogsBackend;

impl AlbumIdentifier for DiscogsBackend {
    fn name(&self) -> &str {
        "Discogs"
    }

    fn find_album_side(
        &self,
        songs: &[IdentifiedSong],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<AlbumSideResult>, Box<dyn Error>> {
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

    fn find_album(
        &self,
        songs: &[IdentifiedSong],
        file_duration_seconds: f64,
        verbose: bool,
    ) -> Result<Option<AlbumResult>, Box<dyn Error>> {
        let release = match discogs::find_album_by_songs(
            songs,
            file_duration_seconds,
            true, // vinyl_only
            verbose,
        )? {
            Some(r) => r,
            None => return Ok(None),
        };

        let sides: Vec<SideInfo> = release.sides.iter().map(|s| {
            SideInfo {
                label: s.label,
                tracks: discogs::side_to_expected_tracks(s),
                total_duration: s.total_duration,
            }
        }).collect();

        Ok(Some(AlbumResult {
            artist: release.artist,
            album_title: release.title,
            release_info: format!(
                "https://www.discogs.com/release/{}",
                release.release_id
            ),
            sides,
            backend: "Discogs".to_string(),
        }))
    }
}
