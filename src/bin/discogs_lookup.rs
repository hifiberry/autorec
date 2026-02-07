//! Test binary for Discogs album lookup from identified songs.
//!
//! Simulates the full cue_creator flow:
//!   1. Read identified songs from a WAV file (using songrec cache)
//!   2. Search Discogs for the album
//!   3. Fetch release tracklist with per-side data
//!   4. Match the best side
//!
//! Usage:
//!   discogs_lookup <WAV_FILE> [--verbose]
//!   discogs_lookup --songs "artist|title|album,artist|title|album,..." --duration <secs> [--verbose]

use autorec::album_identifier::IdentifiedSong;
use autorec::discogs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

    // Parse mode: either from WAV file or from explicit songs
    let (songs, duration) = if let Some(idx) = args.iter().position(|a| a == "--songs") {
        let songs_str = args.get(idx + 1).expect("--songs requires a value");
        let dur_idx = args.iter().position(|a| a == "--duration").expect("--duration required with --songs");
        let duration: f64 = args.get(dur_idx + 1).expect("--duration requires a value")
            .parse().expect("--duration must be a number");
        let songs = parse_songs(songs_str);
        (songs, duration)
    } else {
        // WAV file mode — identify songs using songrec (with cache)
        let wav_file = args.get(1).expect("Usage: discogs_lookup <WAV_FILE> [--verbose]");
        
        println!("Identifying songs in {}...", wav_file);
        let (result, _log) = autorec::album_identifier::identify_songs(wav_file, None);
        let songs = result.expect("Song identification failed");
        
        // Get duration
        let f = std::fs::File::open(wav_file).expect("Cannot open WAV");
        let mut reader = std::io::BufReader::new(f);
        let header = autorec::wavfile::read_wav_header(&mut reader).expect("Cannot read WAV header");
        let bytes_per_sample = (header.bits_per_sample / 8) as f64;
        let frame_size = bytes_per_sample * header.num_channels as f64;
        let duration = header.data_size as f64 / (header.sample_rate as f64 * frame_size);
        
        (songs, duration)
    };

    println!();
    println!("=== Identified Songs ({}) ===", songs.len());
    for s in &songs {
        println!("  [{:.0}s] {} - {} (album: {})",
                 s.timestamp, s.artist, s.title,
                 s.album.as_deref().unwrap_or("?"));
    }
    println!("File duration: {:.0}s", duration);
    println!();

    // Step 1: Try search_releases (needs auth)
    println!("=== Discogs Lookup ===");
    let has_creds = discogs::has_credentials();
    println!("Credentials: {}", if has_creds { "yes" } else { "no" });

    let mut rl = discogs::create_rate_limiter(has_creds);

    // Determine artist + album from songs
    let artist = most_common(&songs.iter().map(|s| s.artist.clone()).collect::<Vec<_>>());
    let albums: Vec<String> = songs.iter().filter_map(|s| s.album.clone()).collect();
    let album = if albums.is_empty() { "".to_string() } else { most_common(&albums) };

    println!("Artist: {}", artist);
    println!("Album:  {}", album);
    println!();

    // Strategy 1: Search by "artist album" if we have credentials
    if has_creds {
        println!("--- Strategy 1: Search by artist + album ---");
        let query = if album.is_empty() || album == "Unknown" {
            artist.clone()
        } else {
            format!("{} {}", artist, album)
        };
        println!("Query: \"{}\"", query);

        match discogs::search_releases(&query, Some("release"), Some("Vinyl"), &mut rl) {
            Ok(results) => {
                println!("Found {} results", results.len());
                for (i, r) in results.iter().take(10).enumerate() {
                    println!("  {}. id={} \"{}\" format={:?} vinyl={} master={:?}",
                             i + 1, r.release_id, r.title, r.format, r.is_vinyl, r.master_id);
                }

                // Fetch top vinyl results and score
                println!();
                let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();

                let vinyl_results: Vec<&discogs::DiscogsSearchResult> = results.iter()
                    .filter(|r| r.is_vinyl)
                    .take(5)
                    .collect();

                println!("Fetching top {} vinyl releases...", vinyl_results.len());
                for r in &vinyl_results {
                    match discogs::fetch_release(r.release_id, &mut rl) {
                        Ok(release) => {
                            println!();
                            println!("  Release {}: {} - {} ({})",
                                     release.release_id, release.artist, release.title,
                                     release.year.map_or("?".into(), |y| y.to_string()));
                            println!("  Sides: {}", release.sides.len());

                            if let Some(side) = discogs::find_best_side(&release, duration, &song_titles, verbose) {
                                println!("  Best side: {} ({:.0}s, {} tracks)",
                                         side.label, side.total_duration, side.tracks.len());
                                for t in &side.tracks {
                                    println!("    {} {} ({:.0}s)", t.position, t.title, t.duration_secs);
                                }
                            } else {
                                println!("  No matching side found");
                            }
                        }
                        Err(e) => {
                            println!("  Failed to fetch {}: {}", r.release_id, e);
                        }
                    }
                }
            }
            Err(e) => {
                println!("Search failed: {}", e);
            }
        }

        println!();
    }

    // Strategy 2: Search by artist name, find master, get vinyl versions
    println!("--- Strategy 2: Search by artist, find master ---");
    let query = if album.is_empty() || album == "Unknown" {
        artist.clone()
    } else {
        format!("{} {}", artist, album)
    };
    println!("Query: \"{}\"", query);

    if has_creds {
        match discogs::search_releases(&query, Some("master"), None, &mut rl) {
            Ok(results) => {
                println!("Found {} master results", results.len());
                for (i, r) in results.iter().take(5).enumerate() {
                    println!("  {}. id={} \"{}\" master_id={:?}",
                             i + 1, r.release_id, r.title, r.master_id);
                }

                // Take the first result's master_id and look up vinyl versions
                if let Some(first) = results.first() {
                    let master_id = first.master_id.unwrap_or(first.release_id);
                    println!();
                    println!("Fetching vinyl versions of master {}...", master_id);

                    match discogs::fetch_master_vinyl_versions(master_id, &mut rl) {
                        Ok(versions) => {
                            println!("Found {} vinyl versions", versions.len());
                            for (i, v) in versions.iter().take(5).enumerate() {
                                println!("  {}. id={} \"{}\" format={:?} year={:?}",
                                         i + 1, v.release_id, v.title, v.format, v.year);
                            }

                            // Fetch the first 2LP version and test
                            let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();
                            for v in versions.iter().take(3) {
                                println!();
                                match discogs::fetch_release(v.release_id, &mut rl) {
                                    Ok(release) => {
                                        println!("  Release {}: {} - {} ({})",
                                                 release.release_id, release.artist, release.title,
                                                 release.year.map_or("?".into(), |y| y.to_string()));
                                        println!("  Sides: {} vinyl={}", release.sides.len(), release.is_vinyl);

                                        if let Some(side) = discogs::find_best_side(&release, duration, &song_titles, verbose) {
                                            println!("  ✓ Best side: {} ({:.0}s, {} tracks)",
                                                     side.label, side.total_duration, side.tracks.len());
                                            for t in &side.tracks {
                                                println!("    {} {} ({:.0}s)", t.position, t.title, t.duration_secs);
                                            }
                                        } else {
                                            println!("  ✗ No matching side");
                                        }
                                    }
                                    Err(e) => {
                                        println!("  Failed to fetch {}: {}", v.release_id, e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            println!("Failed to fetch versions: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                println!("Master search failed: {}", e);
            }
        }
    } else {
        println!("Skipped — requires Discogs authentication");
    }

    println!();
    println!("=== Done ===");
}

fn parse_songs(s: &str) -> Vec<IdentifiedSong> {
    s.split(',')
        .enumerate()
        .map(|(i, entry)| {
            let parts: Vec<&str> = entry.split('|').collect();
            IdentifiedSong {
                timestamp: (i as f64) * 240.0 + 60.0,
                artist: parts.first().unwrap_or(&"Unknown").to_string(),
                title: parts.get(1).unwrap_or(&"Unknown").to_string(),
                album: parts.get(2).map(|s| s.to_string()),
            }
        })
        .collect()
}

fn most_common(items: &[String]) -> String {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in items {
        *counts.entry(item.as_str()).or_default() += 1;
    }
    counts.into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}
