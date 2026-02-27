//! Identify which album a set of WAV files belong to.
//!
//! Pools Shazam-identified songs from all input files, then uses the
//! [`AlbumIdentifier`] trait backends to find the full album (all sides).
//! Once the album is known, assigns each file to the best-matching side
//! using a greedy algorithm based on song-title overlap and duration.
//!
//! Usage:
//!     identify_album [--verbose] [--no-musicbrainz] [--no-discogs] file1.wav file2.wav ...

use std::collections::HashSet;
use std::env;
use std::io::BufReader;
use std::process;

use autorec::album_identifier::{self, IdentifiedSong};
use autorec::lookup::{self, AlbumIdentifier, AlbumResult, SideInfo, DiscogsBackend, MusicBrainzBackend};
use autorec::wavfile;

struct FileData {
    path: String,
    songs: Vec<IdentifiedSong>,
    duration: f64,
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let no_musicbrainz = args.iter().any(|a| a == "--no-musicbrainz" || a == "--no-mb");
    let no_discogs = args.iter().any(|a| a == "--no-discogs");

    let wav_files: Vec<&str> = args.iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();

    if wav_files.is_empty() {
        eprintln!("Usage: identify_album [--verbose] [--no-musicbrainz] [--no-discogs] file1.wav ...");
        process::exit(1);
    }

    println!("=== Album Identifier ===");
    println!("Files: {}", wav_files.len());
    println!();

    // ── Step 1: Identify songs in each file ──────────────────────────────
    let mut files: Vec<FileData> = Vec::new();

    for wav_file in &wav_files {
        let duration = match read_wav_duration(wav_file) {
            Some(d) => d,
            None => continue,
        };

        let short_name = short(wav_file);
        println!("Identifying: {} ({:.0}s)", short_name, duration);

        let (result, _log) = album_identifier::identify_songs(wav_file, None);
        let songs = match result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  Song identification failed: {}", e);
                Vec::new()
            }
        };

        println!("  {} song(s) found", songs.len());
        for s in &songs {
            println!("    {} - {}", s.artist, s.title);
        }

        files.push(FileData {
            path: wav_file.to_string(),
            songs,
            duration,
        });
    }
    println!();

    // ── Step 2: Pool all songs, deduplicate ──────────────────────────────
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut pooled: Vec<IdentifiedSong> = Vec::new();

    for file in &files {
        for song in &file.songs {
            let key = (song.artist.to_lowercase(), song.title.to_lowercase());
            if seen.insert(key) {
                pooled.push(song.clone());
            }
        }
    }

    println!("Pooled {} unique song(s) from {} file(s)", pooled.len(), files.len());
    for song in &pooled {
        println!("  {} - {}", song.artist, song.title);
    }
    println!();

    if pooled.is_empty() {
        println!("No songs identified — cannot find album.");
        process::exit(1);
    }

    // ── Step 3: Find the album (all sides) via the trait backends ─────────
    let avg_duration: f64 =
        files.iter().map(|f| f.duration).sum::<f64>() / files.len().max(1) as f64;
    println!("Average file duration: {:.0}s", avg_duration);
    println!();

    let discogs_backend = DiscogsBackend;
    let mb_vinyl = MusicBrainzBackend { vinyl_only: true };
    let mb_all = MusicBrainzBackend { vinyl_only: false };

    let mut backends: Vec<&dyn AlbumIdentifier> = Vec::new();
    if !no_discogs { backends.push(&discogs_backend); }
    if !no_musicbrainz { backends.push(&mb_vinyl); }
    if !no_musicbrainz { backends.push(&mb_all); }

    if backends.is_empty() {
        eprintln!("No backends enabled.");
        process::exit(1);
    }

    let album = match lookup::find_album_with_fallback(&backends, &pooled, avg_duration, verbose) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("No album match found across any backend.");
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    // Print album info
    println!();
    println!("=== Album Found ===");
    println!("Artist: {}", album.artist);
    println!("Album:  {}", album.album_title);
    println!("Source: {} (via {})", album.release_info, album.backend);
    println!("Sides:  {}", album.sides.len());
    for side in &album.sides {
        let dur = if side.total_duration > 0.0 {
            format!("{:.0}s", side.total_duration)
        } else {
            "no durations".to_string()
        };
        println!("  Side {}: {} tracks ({})", side.label, side.tracks.len(), dur);
        for t in &side.tracks {
            println!("    #{} {} ({:.0}s)", t.position, t.title, t.length_seconds);
        }
    }
    println!();

    // ── Step 4: Assign files to sides (greedy) ───────────────────────────
    assign_files_to_sides(&files, &album, verbose);
}

// ── Side assignment ──────────────────────────────────────────────────────────

fn assign_files_to_sides(files: &[FileData], album: &AlbumResult, verbose: bool) {
    let n_files = files.len();
    let n_sides = album.sides.len();

    // Build score matrix
    let mut scores = vec![vec![0.0f64; n_sides]; n_files];

    for (fi, file) in files.iter().enumerate() {
        let song_titles: Vec<String> = file.songs.iter().map(|s| s.title.clone()).collect();
        for (si, side) in album.sides.iter().enumerate() {
            scores[fi][si] = score_file_vs_side(&song_titles, side, file.duration);
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
            let name = short(&file.path);
            let s = if name.len() > 42 { &name[..42] } else { name };
            print!("  {:>42}", s);
            for si in 0..n_sides {
                print!("  {:>6.1}", scores[fi][si]);
            }
            println!();
        }
        println!();
    }

    // Greedy assignment
    let mut assigned_files: HashSet<usize> = HashSet::new();
    let mut assigned_sides: HashSet<usize> = HashSet::new();

    println!("=== Per-file side assignment ===");
    println!();

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

        let (fi, si, score) = best;
        let side = &album.sides[si];
        let name = short(&files[fi].path);

        let expected_dur: f64 = side.tracks.iter().map(|t| t.length_seconds).sum();
        let error_pct = if files[fi].duration > 0.0 && expected_dur > 0.0 {
            ((expected_dur - files[fi].duration).abs() / files[fi].duration) * 100.0
        } else {
            f64::NAN
        };

        println!("{}", name);
        println!("  → Side {} (score {:.1})", side.label, score);
        println!("  Duration: {:.0}s file, {:.0}s expected ({:.1}% error)",
                 files[fi].duration, expected_dur, error_pct);
        println!("  Tracks:");
        for t in &side.tracks {
            println!("    #{} {} ({:.0}s)", t.position, t.title, t.length_seconds);
        }
        println!();

        assigned_files.insert(fi);
        assigned_sides.insert(si);
    }

    // Report unassigned files
    for fi in 0..n_files {
        if !assigned_files.contains(&fi) {
            println!("{}: not assigned to any side", short(&files[fi].path));
        }
    }
}

/// Score a file against a side: song-title overlap + duration match.
fn score_file_vs_side(song_titles: &[String], side: &SideInfo, file_duration: f64) -> f64 {
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

// ── Helpers ──────────────────────────────────────────────────────────────────

fn short(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

fn read_wav_duration(path: &str) -> Option<f64> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => { eprintln!("Cannot open {}: {}", path, e); return None; }
    };
    let mut reader = BufReader::new(f);
    match wavfile::read_wav_header(&mut reader) {
        Ok(h) => {
            let bps = (h.bits_per_sample / 8) as f64;
            let frame = bps * h.num_channels as f64;
            Some(h.data_size as f64 / (h.sample_rate as f64 * frame))
        }
        Err(e) => { eprintln!("Bad WAV header {}: {}", path, e); None }
    }
}
