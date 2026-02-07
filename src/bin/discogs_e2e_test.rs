//! End-to-end test: find_album_by_songs → find_best_side for all 4 DJ Shadow sides.

use autorec::album_identifier::IdentifiedSong;
use autorec::discogs;

fn main() {
    println!("=== Discogs find_album_by_songs — 4 Side Test ===\n");

    let test_cases: Vec<(&str, f64, Vec<(&str, &str, &str)>, char)> = vec![
        ("Side 1", 1333.0, vec![
            ("DJ Shadow", "Building Steam With a Grain of Salt", "Endtroducing....."),
            ("DJ Shadow", "The Number Song", "Endtroducing....."),
            ("DJ Shadow", "Changeling / Transmission", "Endtroducing....."),
        ], 'A'),
        ("Side 2", 1049.0, vec![
            ("DJ Shadow", "What Does Your Soul Look Like, Pt. 4", "Endtroducing....."),
            ("DJ Shadow", "Stem/Long Stem", "Endtroducing....."),
        ], 'B'),
        ("Side 3", 793.0, vec![
            ("DJ Shadow", "Mutual Slump", "Endtroducing....."),
            ("DJ Shadow", "Organ Donor", "Endtroducing....."),
            ("DJ Shadow", "Midnight In a Perfect World", "Endtroducing....."),
        ], 'C'),
        ("Side 4", 1130.0, vec![
            ("DJ Shadow", "Napalm Brain / Scatter Brain", "Endtroducing....."),
            ("DJ Shadow", "What Does Your Soul Look Like Pt. 1 / Blue Sky Revisit / Transmission 3", "Endtroducing....."),
        ], 'D'),
    ];

    let mut pass = 0;
    let mut fail = 0;

    for (label, duration, song_data, expected_side) in &test_cases {
        println!("--- {} (duration={:.0}s, expected side {}) ---", label, duration, expected_side);

        let songs: Vec<IdentifiedSong> = song_data.iter().enumerate()
            .map(|(i, (artist, title, album))| IdentifiedSong {
                timestamp: i as f64 * 240.0 + 60.0,
                artist: artist.to_string(),
                title: title.to_string(),
                album: Some(album.to_string()),
            })
            .collect();

        match discogs::find_album_by_songs(&songs, *duration, true, true) {
            Ok(Some(release)) => {
                println!("  Found: {} - {} (id={}, year={:?})",
                         release.artist, release.title, release.release_id, release.year);

                let song_titles: Vec<String> = songs.iter().map(|s| s.title.clone()).collect();
                if let Some(side) = discogs::find_best_side(&release, *duration, &song_titles, false) {
                    println!("  Best side: {} ({:.0}s, {} tracks)", side.label, side.total_duration, side.tracks.len());
                    for t in &side.tracks {
                        println!("    {} {} ({:.0}s)", t.position, t.title, t.duration_secs);
                    }
                    if side.label == *expected_side {
                        println!("  ✓ CORRECT\n");
                        pass += 1;
                    } else {
                        println!("  ✗ WRONG — expected {}, got {}\n", expected_side, side.label);
                        fail += 1;
                    }
                } else {
                    println!("  ✗ No matching side found\n");
                    fail += 1;
                }
            }
            Ok(None) => {
                println!("  ✗ No release found\n");
                fail += 1;
            }
            Err(e) => {
                println!("  ✗ Error: {}\n", e);
                fail += 1;
            }
        }
    }

    println!("=== Results: {} passed, {} failed ===", pass, fail);
    if fail > 0 {
        std::process::exit(1);
    }
}
