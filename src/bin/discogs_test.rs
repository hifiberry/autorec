//! Quick test to exercise the Discogs module against DJ Shadow Endtroducing sides.
//!
//! Usage: discogs_test [--release-id ID]

use autorec::discogs;
use autorec::rate_limiter::RateLimiter;

fn main() {
    let release_id: u64 = std::env::args()
        .skip_while(|a| a != "--release-id")
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30298511); // DJ Shadow - Endtroducing 25th Anniversary

    println!("=== Discogs Module Test ===");
    println!();

    // Check credentials
    if discogs::has_credentials() {
        println!("✓ Discogs credentials found (60 req/min)");
    } else {
        println!("✗ No Discogs credentials — using unauthenticated (25 req/min)");
    }
    println!();

    // Fetch release
    let mut rl = discogs::create_rate_limiter(discogs::has_credentials());

    println!("Fetching release {}...", release_id);
    let release = match discogs::fetch_release(release_id, &mut rl) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching release: {}", e);
            std::process::exit(1);
        }
    };

    println!("  Title:  {} - {}", release.artist, release.title);
    println!("  Year:   {:?}", release.year);
    println!("  Vinyl:  {}", release.is_vinyl);
    println!("  Sides:  {}", release.sides.len());
    println!();

    for side in &release.sides {
        println!("  Side {} ({:.0}s total, {} tracks):", side.label, side.total_duration, side.tracks.len());
        for track in &side.tracks {
            println!("    {:6} {:50} {:>6.0}s", track.position, track.title, track.duration_secs);
        }
        println!();
    }

    // Test side matching against known file durations
    // These are the DJ Shadow 4-side recordings
    println!("=== Side Matching Test ===");
    println!();

    let test_cases = [
        ("Side 1 (DJ Shadow - Endtroducing .1.wav)", 1333.0, vec![
            "Best Foot Forward", "Building Steam", "Number Song", "Changeling",
        ]),
        ("Side 2 (DJ Shadow - Endtroducing .2.wav)", 1049.0, vec![
            "What Does Your Soul Look Like", "Stem", "Long Stem",
        ]),
        ("Side 3 (DJ Shadow - Endtroducing .3.wav)", 793.0, vec![
            "Mutual Slump", "Organ Donor", "Hip Hop Sucks", "Midnight",
        ]),
        ("Side 4 (DJ Shadow - Endtroducing .4.wav)", 1130.0, vec![
            "Napalm Brain", "Scatter Brain", "What Does Your Soul Look Like",
        ]),
    ];

    for (label, duration, song_titles) in &test_cases {
        let titles: Vec<String> = song_titles.iter().map(|s| s.to_string()).collect();
        let best = discogs::find_best_side(&release, *duration, &titles, false);
        
        match best {
            Some(side) => {
                println!("  {} ({:.0}s):", label, duration);
                println!("    → Matched Side {}: {:.0}s, {} tracks",
                         side.label, side.total_duration, side.tracks.len());
                for t in &side.tracks {
                    println!("      {} {}", t.position, t.title);
                }

                // Check: expected side letter
                let expected = match label {
                    l if l.contains("Side 1") => 'A',
                    l if l.contains("Side 2") => 'B',
                    l if l.contains("Side 3") => 'C',
                    l if l.contains("Side 4") => 'D',
                    _ => '?',
                };
                if side.label == expected {
                    println!("    ✓ CORRECT (Side {})", expected);
                } else {
                    println!("    ✗ WRONG — expected Side {}, got Side {}", expected, side.label);
                }
            }
            None => {
                println!("  {} ({:.0}s): NO MATCH", label, duration);
            }
        }
        println!();
    }

    // Also test ExpectedTrack conversion
    println!("=== ExpectedTrack Conversion (Side A) ===");
    if let Some(side_a) = release.sides.iter().find(|s| s.label == 'A') {
        let expected = discogs::side_to_expected_tracks(side_a);
        for t in &expected {
            println!("  #{}: {} ({:.1}s, starts at {:.1}s)",
                     t.position, t.title, t.length_seconds, t.expected_start);
        }
    }
}
