use autorec::identify_songs;
use std::env;
use std::process;

fn print_usage() {
    println!("Album Identifier - Identify albums from WAV recordings using song recognition");
    println!();
    println!("Usage: album_identifier <WAV_FILE> [OPTIONS]");
    println!();
    println!("Arguments:");
    println!("  WAV_FILE                      Path to the WAV file to analyze");
    println!();
    println!("Options:");
    println!("  --first-timestamp <SECONDS>   First recognition timestamp in seconds (default: 60)");
    println!("  --interval <SECONDS>          Interval between recognitions in seconds (default: 240)");
    println!("  --timestamps <T1,T2...>       Override with specific comma-separated timestamps");
    println!("  --help, -h                    Show this help message");
    println!();
    println!("Examples:");
    println!("  album_identifier recording.1.wav");
    println!("  album_identifier recording.1.wav --first-timestamp 30 --interval 300");
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
    let mut first_timestamp: f64 = 60.0;   // Default 1 minute
    let mut interval: f64 = 240.0;          // Default 4 minutes

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            "--first-timestamp" => {
                if i + 1 < args.len() {
                    first_timestamp = args[i + 1].parse().unwrap_or(60.0);
                    i += 1;
                } else {
                    eprintln!("Error: --first-timestamp requires an argument");
                    process::exit(1);
                }
            }
            "--interval" => {
                if i + 1 < args.len() {
                    interval = args[i + 1].parse().unwrap_or(240.0);
                    i += 1;
                } else {
                    eprintln!("Error: --interval requires an argument");
                    process::exit(1);
                }
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

    // If custom timestamps not provided, generate them using first_timestamp and interval
    let timestamps = if custom_timestamps.is_none() {
        // Assume 30 minute recording for default generation
        Some(autorec::album_identifier::generate_default_timestamps(1800.0, first_timestamp, interval))
    } else {
        custom_timestamps
    };

    // Run song identification
    let (result, _log) = identify_songs(&wav_file, timestamps);
    match result {
        Ok(songs) => {
            println!();
            println!("=== Song Identification Results ===");
            println!();
            println!("Identified Songs: {}", songs.len());
            for (i, song) in songs.iter().enumerate() {
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
