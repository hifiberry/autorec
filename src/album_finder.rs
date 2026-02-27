//! Multi-file album identification.
//!
//! When several WAV files belong to the same vinyl album (e.g. 4 sides of a
//! double LP) this module pools the identified songs from **all** files, finds
//! the album once using all available evidence, and then assigns each file to
//! the correct side – regardless of the file order on disk.
//!
//! # Algorithm
//!
//! 1. **Collect** – for each WAV file, receive a list of Shazam-identified
//!    songs and the music-region duration.
//! 2. **Pool** – merge all songs from all files into a single list, deduplicate
//!    by (artist, title) across files so that the album search sees as many
//!    distinct songs as possible.
//! 3. **Search** – use the pooled songs to query Discogs (then MusicBrainz as
//!    fallback) for the album.  More songs ⇒ more reliable match.
//! 4. **Assign** – for each file, score every side of the found release by both
//!    song-title overlap **and** duration match, then pick the best (file, side)
//!    assignment using a greedy algorithm.  This handles the case where file 1
//!    is actually side B and file 2 is side A.

use std::collections::{HashMap, HashSet};
use std::error::Error;

use crate::album_identifier::IdentifiedSong;
use crate::discogs::{self, DiscogsRelease, DiscogsSide};
use crate::musicbrainz::{self, ExpectedTrack};
use crate::rate_limiter::RateLimiter;

// ── Input / output types ─────────────────────────────────────────────────────

/// Per-file input to the album finder.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Path to the WAV file
    pub path: String,
    /// Songs identified by Shazam for this file
    pub songs: Vec<IdentifiedSong>,
    /// Duration of the music region (groove-in to groove-out) in seconds
    pub music_duration: f64,
}

/// Per-file result after side assignment.
#[derive(Debug, Clone)]
pub struct FileSideResult {
    /// Original file path
    pub path: String,
    /// Artist name
    pub artist: String,
    /// Album title
    pub album_title: String,
    /// Human-readable release reference (URL)
    pub release_info: String,
    /// Which side letter was assigned (e.g. 'A', 'B', 'C', 'D')
    pub side_label: char,
    /// Ordered track list for the assigned side
    pub tracks: Vec<ExpectedTrack>,
    /// Name of the backend that found the album
    pub backend: String,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Find the album for a group of files that are believed to be from the same
/// record, then assign each file to its correct side.
///
/// `no_discogs` / `no_musicbrainz` control which backends to try.
///
/// Returns `Ok(None)` when no album could be identified.
/// Returns `Ok(Some(vec))` with one entry per input file (same order).
pub fn find_album_for_files(
    files: &[FileInfo],
    no_discogs: bool,
    no_musicbrainz: bool,
    verbose: bool,
) -> Result<Option<Vec<FileSideResult>>, Box<dyn Error>> {
    if files.is_empty() {
        return Ok(None);
    }

    // ── Step 1: Pool all identified songs ────────────────────────────────
    let pooled = pool_songs(files);
    println!("Pooled {} unique song(s) from {} file(s)", pooled.len(), files.len());
    for song in &pooled {
        println!("  {} - {}", song.artist, song.title);
    }
    println!();

    if pooled.is_empty() {
        println!("No songs identified across any file");
        return Ok(None);
    }

    // Average duration per file — Discogs scores each *side* against this
    // value, so we want a single-side estimate, not the total of all files.
    let avg_duration: f64 =
        files.iter().map(|f| f.music_duration).sum::<f64>() / files.len() as f64;

    // ── Step 2: Find the album on Discogs ────────────────────────────────
    let mut discogs_release: Option<DiscogsRelease> = None;

    if !no_discogs {
        println!("Searching Discogs with all songs (avg side duration {:.0}s)...", avg_duration);
        match discogs::find_album_by_songs(&pooled, avg_duration, true, verbose)? {
            Some(release) => {
                println!("Discogs: found {} - {} ({} sides)",
                         release.artist, release.title, release.sides.len());
                for side in &release.sides {
                    let dur_str = if side.total_duration > 0.0 {
                        format!("{:.0}s", side.total_duration)
                    } else {
                        "no durations".to_string()
                    };
                    println!("  Side {}: {} tracks ({})", side.label, side.tracks.len(), dur_str);
                }
                println!();
                discogs_release = Some(release);
            }
            None => {
                println!("Discogs: no match found");
                println!();
            }
        }
    }

    // ── Step 3: If we have a Discogs release, assign files to sides ──────
    if let Some(ref release) = discogs_release {
        let assignments = assign_files_to_sides(files, release, verbose);

        if !assignments.is_empty() {
            // Check if all sides have usable durations; if not, enrich from MB
            let needs_enrichment = assignments.iter()
                .any(|(_, side)| side.total_duration <= 0.0);

            let mut mb_tracks_by_side: HashMap<char, Vec<ExpectedTrack>> = HashMap::new();

            if needs_enrichment && !no_musicbrainz {
                println!("Some sides have no duration data, enriching from MusicBrainz...");
                if let Some(enriched) = enrich_from_musicbrainz(
                    &release.artist, &release.title, release, verbose,
                )? {
                    mb_tracks_by_side = enriched;
                }
                println!();
            }

            // Build results
            let mut results: Vec<Option<FileSideResult>> = vec![None; files.len()];

            for (file_idx, side) in &assignments {
                let tracks = if let Some(mb_tracks) = mb_tracks_by_side.get(&side.label) {
                    // Use MusicBrainz durations
                    mb_tracks.clone()
                } else {
                    discogs::side_to_expected_tracks(side)
                };

                let backend = if mb_tracks_by_side.contains_key(&side.label) {
                    "Discogs + MusicBrainz (durations)".to_string()
                } else {
                    "Discogs".to_string()
                };

                results[*file_idx] = Some(FileSideResult {
                    path: files[*file_idx].path.clone(),
                    artist: release.artist.clone(),
                    album_title: release.title.clone(),
                    release_info: format!(
                        "https://www.discogs.com/release/{}",
                        release.release_id,
                    ),
                    side_label: side.label,
                    tracks,
                    backend,
                });
            }

            // Collect results, filtering out files that couldn't be assigned
            let final_results: Vec<FileSideResult> = results.into_iter()
                .enumerate()
                .map(|(i, r)| r.unwrap_or_else(|| FileSideResult {
                    path: files[i].path.clone(),
                    artist: release.artist.clone(),
                    album_title: release.title.clone(),
                    release_info: format!(
                        "https://www.discogs.com/release/{}",
                        release.release_id,
                    ),
                    side_label: '?',
                    tracks: Vec::new(),
                    backend: "Discogs (no side matched)".to_string(),
                }))
                .collect();

            return Ok(Some(final_results));
        }
    }

    // ── Step 4: Fallback to MusicBrainz ──────────────────────────────────
    if !no_musicbrainz {
        println!("Trying MusicBrainz with all songs...");
        if let Some(result) = find_via_musicbrainz(files, &pooled, avg_duration, verbose)? {
            return Ok(Some(result));
        }
    }

    Ok(None)
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Merge songs from all files, deduplicate by (artist, title) case-insensitively.
/// Keeps one representative IdentifiedSong per unique (artist, title).
fn pool_songs(files: &[FileInfo]) -> Vec<IdentifiedSong> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut pooled = Vec::new();

    for file in files {
        for song in &file.songs {
            let key = (song.artist.to_lowercase(), song.title.to_lowercase());
            if seen.insert(key) {
                pooled.push(song.clone());
            }
        }
    }

    pooled
}

/// Assign each file to the best matching Discogs side using a greedy algorithm.
///
/// For each (file, side) pair, compute a score based on song-title overlap and
/// (optionally) duration match.  Then greedily pick the best pair, remove both
/// the file and the side from the pool, and repeat.
///
/// Returns a list of (file_index, &DiscogsSide) assignments.
fn assign_files_to_sides<'a>(
    files: &[FileInfo],
    release: &'a DiscogsRelease,
    verbose: bool,
) -> Vec<(usize, &'a DiscogsSide)> {
    if release.sides.is_empty() {
        return Vec::new();
    }

    // Build score matrix: score[file_idx][side_idx]
    let n_files = files.len();
    let n_sides = release.sides.len();
    let mut scores = vec![vec![0.0f64; n_sides]; n_files];

    for (fi, file) in files.iter().enumerate() {
        let song_titles: Vec<String> = file.songs.iter()
            .map(|s| s.title.clone())
            .collect();

        for (si, side) in release.sides.iter().enumerate() {
            scores[fi][si] = score_file_vs_side(file, side, &song_titles);
        }
    }

    if verbose {
        println!("Assignment score matrix:");
        print!("  {:>40}", "");
        for side in &release.sides {
            print!("  Side {} ", side.label);
        }
        println!();
        for (fi, file) in files.iter().enumerate() {
            let name = std::path::Path::new(&file.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&file.path);
            let short = if name.len() > 40 { &name[..40] } else { name };
            print!("  {:>40}", short);
            for si in 0..n_sides {
                print!("  {:>6.1}", scores[fi][si]);
            }
            println!();
        }
        println!();
    }

    // Greedy assignment: pick highest score, assign, remove both from pool
    let mut assigned_files: HashSet<usize> = HashSet::new();
    let mut assigned_sides: HashSet<usize> = HashSet::new();
    let mut assignments: Vec<(usize, &'a DiscogsSide)> = Vec::new();

    let pairs_to_assign = n_files.min(n_sides);
    for _ in 0..pairs_to_assign {
        let mut best_fi = 0;
        let mut best_si = 0;
        let mut best_score = f64::NEG_INFINITY;

        for fi in 0..n_files {
            if assigned_files.contains(&fi) { continue; }
            for si in 0..n_sides {
                if assigned_sides.contains(&si) { continue; }
                if scores[fi][si] > best_score {
                    best_score = scores[fi][si];
                    best_fi = fi;
                    best_si = si;
                }
            }
        }

        if best_score <= 0.0 {
            break; // No more useful assignments
        }

        let side = &release.sides[best_si];
        let name = std::path::Path::new(&files[best_fi].path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&files[best_fi].path);
        println!("  {} → Side {} (score {:.1})", name, side.label, best_score);

        assigned_files.insert(best_fi);
        assigned_sides.insert(best_si);
        assignments.push((best_fi, side));
    }

    println!();
    assignments
}

/// Score a file against a Discogs side based on song-title overlap and
/// (when available) duration match.
fn score_file_vs_side(file: &FileInfo, side: &DiscogsSide, song_titles: &[String]) -> f64 {
    if side.tracks.is_empty() || song_titles.is_empty() {
        return 0.0;
    }

    // ── Song title overlap ───────────────────────────────────────────────
    let track_titles_lower: Vec<String> = side.tracks.iter()
        .map(|t| t.title.to_lowercase())
        .collect();

    let mut song_matches = 0;
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

    let max_songs = song_titles.len().max(1) as f64;
    let song_score = song_matches as f64 / max_songs;

    // ── Duration match ───────────────────────────────────────────────────
    let duration_score = if side.total_duration > 0.0 {
        let duration_error = (side.total_duration - file.music_duration).abs();
        let duration_ratio = duration_error / file.music_duration;
        (1.0 - duration_ratio * 10.0).max(0.0)
    } else {
        // No duration data → neutral (don't penalise, don't reward)
        0.5
    };

    // Combined: song overlap is more important
    song_score * 100.0 + duration_score * 10.0
}

/// Try to get duration data from MusicBrainz for a Discogs album.
///
/// Searches MusicBrainz for the same artist + album title, flattens all
/// tracks, then for each Discogs side's tracks finds the best title-match
/// in the MB pool to obtain durations.
///
/// This avoids having to map MB media to Discogs sides (which is fragile
/// because a single MB medium often contains two physical vinyl sides).
///
/// Returns a map of side-label → Vec<ExpectedTrack> for sides where durations
/// were successfully obtained.
fn enrich_from_musicbrainz(
    artist: &str,
    album_title: &str,
    discogs_release: &DiscogsRelease,
    verbose: bool,
) -> Result<Option<HashMap<char, Vec<ExpectedTrack>>>, Box<dyn Error>> {
    let mut rl = RateLimiter::from_millis("MusicBrainz", 1100);

    let results = musicbrainz::search_release(artist, album_title, 10)?;
    rl.wait_if_needed();

    if results.is_empty() {
        if verbose {
            println!("  MusicBrainz: no releases found for enrichment");
        }
        return Ok(None);
    }

    // Prefer vinyl releases, fall back to all
    let vinyl: Vec<_> = results.iter().filter(|r| r.is_vinyl).cloned().collect();
    let candidates = if vinyl.is_empty() { &results } else { &vinyl };

    // Count total Discogs tracks so we can judge match quality
    let total_discogs_tracks: usize = discogs_release.sides.iter()
        .map(|s| s.tracks.len())
        .sum();

    for result in candidates.iter().take(5) {
        let mb_sides = match musicbrainz::fetch_release_sides(&result.release_id) {
            Ok(s) => s,
            Err(_) => { rl.wait_if_needed(); continue; }
        };
        rl.wait_if_needed();

        // Flatten all MB tracks into one pool
        let all_mb_tracks: Vec<&ExpectedTrack> = mb_sides.iter()
            .flat_map(|s| s.tracks.iter())
            .collect();

        // Check that this release has actual duration data
        let total_dur: f64 = all_mb_tracks.iter().map(|t| t.length_seconds).sum();
        if total_dur <= 0.0 {
            continue;
        }

        // For each Discogs side, match each track by title to the best MB track
        let mut result_map: HashMap<char, Vec<ExpectedTrack>> = HashMap::new();
        let mut used_mb_indices: HashSet<usize> = HashSet::new();
        let mut total_matched: usize = 0;

        for discogs_side in &discogs_release.sides {
            let mut side_tracks: Vec<ExpectedTrack> = Vec::new();
            let mut cumulative = 0.0;

            for dt in &discogs_side.tracks {
                let dt_lower = dt.title.to_lowercase();
                let dt_words: Vec<&str> = dt_lower.split_whitespace()
                    .filter(|w| w.len() >= 3)
                    .collect();

                // Find best matching MB track that hasn't been used yet
                let mut best_idx: Option<usize> = None;
                let mut best_word_matches = 0usize;

                for (mi, mb_track) in all_mb_tracks.iter().enumerate() {
                    if used_mb_indices.contains(&mi) { continue; }
                    let mb_lower = mb_track.title.to_lowercase();
                    let word_matches = dt_words.iter()
                        .filter(|w| mb_lower.contains(**w))
                        .count();
                    if word_matches >= 1
                        && (word_matches as f64 / dt_words.len().max(1) as f64) >= 0.3
                        && word_matches > best_word_matches
                    {
                        best_word_matches = word_matches;
                        best_idx = Some(mi);
                    }
                }

                if let Some(mi) = best_idx {
                    let mb_track = all_mb_tracks[mi];
                    side_tracks.push(ExpectedTrack {
                        position: dt.position.chars()
                            .filter(|c| c.is_ascii_digit())
                            .collect::<String>()
                            .parse()
                            .unwrap_or(0),
                        title: dt.title.clone(), // keep Discogs title
                        length_seconds: mb_track.length_seconds,
                        expected_start: cumulative,
                    });
                    cumulative += mb_track.length_seconds;
                    used_mb_indices.insert(mi);
                    total_matched += 1;
                } else {
                    // No MB match for this track — insert with 0 duration
                    side_tracks.push(ExpectedTrack {
                        position: dt.position.chars()
                            .filter(|c| c.is_ascii_digit())
                            .collect::<String>()
                            .parse()
                            .unwrap_or(0),
                        title: dt.title.clone(),
                        length_seconds: 0.0,
                        expected_start: cumulative,
                    });
                }
            }

            if !side_tracks.is_empty() {
                if verbose {
                    let dur: f64 = side_tracks.iter().map(|t| t.length_seconds).sum();
                    let matched = side_tracks.iter().filter(|t| t.length_seconds > 0.0).count();
                    println!("  MusicBrainz: Side {} — {}/{} tracks matched ({:.0}s)",
                             discogs_side.label, matched, side_tracks.len(), dur);
                }
                result_map.insert(discogs_side.label, side_tracks);
            }
        }

        // Only accept if we matched a decent fraction of tracks
        let match_fraction = total_matched as f64 / total_discogs_tracks.max(1) as f64;
        if total_matched > 0 && match_fraction >= 0.5 {
            if verbose {
                println!("  MusicBrainz: enriched {} side(s), matched {}/{} tracks from release {}",
                         result_map.len(), total_matched, total_discogs_tracks, result.release_id);
            }
            return Ok(Some(result_map));
        } else if verbose {
            println!("  MusicBrainz: only matched {}/{} tracks ({:.0}%) from release {} — skipping",
                     total_matched, total_discogs_tracks, match_fraction * 100.0, result.release_id);
        }
    }

    Ok(None)
}

/// Count how many titles from `source_titles` match titles in `tracks`.
fn count_title_overlap_tracks(source_titles: &[String], tracks: &[ExpectedTrack]) -> usize {
    let track_titles_lower: Vec<String> = tracks.iter()
        .map(|t| t.title.to_lowercase())
        .collect();

    let mut matches = 0;
    for title in source_titles {
        let title_lower = title.to_lowercase();
        let words: Vec<&str> = title_lower.split_whitespace()
            .filter(|w| w.len() >= 3)
            .collect();

        for track_title in &track_titles_lower {
            let word_matches = words.iter()
                .filter(|w| track_title.contains(**w))
                .count();
            if word_matches >= 1
                && (word_matches as f64 / words.len().max(1) as f64) >= 0.3
            {
                matches += 1;
                break;
            }
        }
    }

    matches
}

/// Rebuild expected_start values from a slice of tracks (cumulative from 0).
fn rebuild_expected_starts(tracks: &[ExpectedTrack]) -> Vec<ExpectedTrack> {
    let mut cumulative = 0.0;
    tracks.iter()
        .map(|t| {
            let et = ExpectedTrack {
                position: t.position,
                title: t.title.clone(),
                length_seconds: t.length_seconds,
                expected_start: cumulative,
            };
            cumulative += t.length_seconds;
            et
        })
        .collect()
}

/// Fallback: try MusicBrainz for the whole album using pooled songs.
fn find_via_musicbrainz(
    files: &[FileInfo],
    pooled_songs: &[IdentifiedSong],
    _total_duration: f64,
    verbose: bool,
) -> Result<Option<Vec<FileSideResult>>, Box<dyn Error>> {
    // Try vinyl first, then all
    for vinyl_only in [true, false] {
        let label = if vinyl_only { "MusicBrainz (vinyl)" } else { "MusicBrainz (all)" };
        println!("Trying {}...", label);

        // Use the first file's duration as a rough guide for the search
        let avg_duration = files.iter().map(|f| f.music_duration).sum::<f64>() / files.len() as f64;

        let (best, _) = match musicbrainz::find_album_by_songs(
            pooled_songs, avg_duration, vinyl_only, verbose,
        )? {
            Some(r) => r,
            None => { println!("{}: no match", label); continue; }
        };

        println!("{}: found {} - {}", label, best.artist, best.title);

        let sides = musicbrainz::fetch_release_sides(&best.release_id)?;
        if sides.is_empty() {
            continue;
        }

        // Assign files to MB sides using greedy matching
        let mut results: Vec<Option<FileSideResult>> = vec![None; files.len()];
        let mut assigned_sides: HashSet<u32> = HashSet::new();

        // Score each (file, mb_side) pair
        let mut all_pairs: Vec<(usize, usize, f64)> = Vec::new();
        for (fi, file) in files.iter().enumerate() {
            let song_titles: Vec<String> = file.songs.iter()
                .map(|s| s.title.clone())
                .collect();
            for (si, mb_side) in sides.iter().enumerate() {
                let overlap = count_title_overlap_tracks(&song_titles, &mb_side.tracks);
                let dur_score = if mb_side.total_duration > 0.0 {
                    let ratio = (mb_side.total_duration - file.music_duration).abs() / file.music_duration;
                    (1.0 - ratio * 10.0).max(0.0)
                } else { 0.5 };
                let score = overlap as f64 * 100.0 + dur_score * 10.0;
                all_pairs.push((fi, si, score));
            }
        }

        all_pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        let mut assigned_files: HashSet<usize> = HashSet::new();
        let mut assigned_side_idxs: HashSet<usize> = HashSet::new();

        for (fi, si, score) in &all_pairs {
            if assigned_files.contains(fi) || assigned_side_idxs.contains(si) { continue; }

            let mb_side = &sides[*si];
            let tracks = rebuild_expected_starts(&mb_side.tracks);

            let name = std::path::Path::new(&files[*fi].path)
                .file_name().and_then(|n| n.to_str()).unwrap_or(&files[*fi].path);
            println!("  {} → Medium {} (score {:.1})", name, mb_side.position, score);

            results[*fi] = Some(FileSideResult {
                path: files[*fi].path.clone(),
                artist: best.artist.clone(),
                album_title: best.title.clone(),
                release_info: format!(
                    "https://musicbrainz.org/release/{}",
                    best.release_id,
                ),
                side_label: ('A' as u8 + mb_side.position.saturating_sub(1) as u8) as char,
                tracks,
                backend: label.to_string(),
            });

            assigned_files.insert(*fi);
            assigned_side_idxs.insert(*si);
        }

        // Fill unassigned files with empty results
        let final_results: Vec<FileSideResult> = results.into_iter()
            .enumerate()
            .map(|(i, r)| r.unwrap_or_else(|| FileSideResult {
                path: files[i].path.clone(),
                artist: best.artist.clone(),
                album_title: best.title.clone(),
                release_info: format!(
                    "https://musicbrainz.org/release/{}",
                    best.release_id,
                ),
                side_label: '?',
                tracks: Vec::new(),
                backend: format!("{} (no side matched)", label),
            }))
            .collect();

        return Ok(Some(final_results));
    }

    Ok(None)
}
