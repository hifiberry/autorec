use autorec::identify_album;
use std::env;
use std::process;

fn print_usage() {
    println!("Album Identifier - Identify albums from WAV recordings using song recognition");
    println!();
    println!("Usage: album_identifier <WAV_FILE> [OPTIONS]");
    println!();
    println!("Arguments:");
    println!("  WAV_FILE              Path to the WAV file to analyze");
    println!();
    println!("Options:");
    println!("  --timestamps <T1,T2...>  Comma-separated timestamps in seconds (default: 60, then every 360s)");
    println!("  --help, -h               Show this help message");
    println!();
    println!("Examples:");
    println!("  album_identifier recording.1.wav");
    println!("  album_identifier recording.1.wav --timestamps 60,420,780");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Error: No WAV file specified");
        println!();
        print_usage();
        process::exit(1);
    }

    let mut wav_file = String::new();
    let mut custom_timestamps: Option<Vec<f64>> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            "--timestamps" => {
                if i + 1 < args.len() {
                    let timestamps_str = &args[i + 1];
                    let timestamps: Vec<f64> = timestamps_str
                        .split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect();
                    
                    if timestamps.is_empty() {
                        eprintln!("Error: Invalid timestamps format");
                        process::exit(1);
                    }
                    
                    custom_timestamps = Some(timestamps);
                    i += 1;
                } else {
                    eprintln!("Error: --timestamps requires an argument");
                    process::exit(1);
                }
            }
            arg if !arg.starts_with("--") => {
                wav_file = arg.to_string();
            }
            _ => {
                eprintln!("Error: Unknown option: {}", args[i]);
                print_usage();
                process::exit(1);
            }
        }
        i += 1;
    }

    if wav_file.is_empty() {
        eprintln!("Error: No WAV file specified");
        print_usage();
        process::exit(1);
    }

    // Run album identification
    match identify_album(&wav_file, custom_timestamps) {
        Ok(album_info) => {
            println!();
            println!("=== Album Identification Results ===");
            println!();
            println!("Album:  {}", album_info.album_title);
            println!("Artist: {}", album_info.album_artist);
            println!("Confidence: {:.0}%", album_info.confidence * 100.0);
            println!();
            println!("Identified Songs:");
            for (i, song) in album_info.songs.iter().enumerate() {
                let mins = (song.timestamp / 60.0) as u32;
                let secs = (song.timestamp % 60.0) as u32;
                println!("  {}. [{}:{:02}] {} - {}", 
                    i + 1, mins, secs, song.artist, song.title);
                if let Some(ref album) = song.album {
                    println!("      Album: {}", album);
                }
            }
            println!();
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
}
