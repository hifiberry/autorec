use autorec::{create_input_stream, display_vu_meter, list_targets, parse_audio_address, process_audio_chunk, validate_and_select_target, AudioRecorder, SampleFormat, VUMeter};
use std::env;
use std::process;
use std::thread;
use std::time::Duration;

fn print_usage() {
    println!("Audio recording program with automatic start/stop based on signal detection");
    println!();
    println!("Usage: record [FILENAME] [OPTIONS]");
    println!();
    println!("Arguments:");
    println!("  FILENAME                 Base filename for recordings (default: recording)");
    println!();
    println!("Options:");
    println!("  --list-targets           List available PipeWire recording targets and exit");
    println!("  --source <SOURCE>        Audio source address:");
    println!("                             pipewire:device or pw:device");
    println!("                             alsa:hw:0,0 or alsa:default");
    println!("                             file:path/to/audio.wav");
    println!("                             /path/to/audio.mp3 (auto-detects as file)");
    println!("                             Auto-detects backend if not specified");
    println!("                             (default: auto-detect PipeWire source)");
    println!("  --rate <RATE>            Sample rate (default: 96000)");
    println!("  --channels <CHANNELS>    Number of channels (default: 2)");
    println!("  --format <FORMAT>        Sample format: s16, s32 (default: s32)");
    println!("  --interval <INTERVAL>    Update interval in seconds (default: 0.2)");
    println!("  --db-range <RANGE>       dB range to display (default: 90)");
    println!("  --max-db <MAX>           Maximum dB (default: 0)");
    println!("  --off-threshold <THRESH> Threshold for on/off detection in dB (default: -60)");
    println!("  --silence-duration <SEC> Duration of silence before recording stops (default: 10)");
    println!("  --min-length <SEC>       Minimum recording length in seconds (default: 600)");
    println!("  --no-vumeter             Disable VU meter display (simple text output)");
    println!("  --help                   Show this help message");
    println!();
    println!("Examples:");
    println!("  record vinyl --source pipewire:riaa.monitor");
    println!("  record tape --source alsa:hw:1,0 --rate 48000");
    println!("  record test --source /path/to/source.flac");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Default values
    let mut record_file = "recording".to_string();
    let mut source: Option<String> = None;
    let mut rate = 96000;
    let mut channels = 2;
    let mut format = SampleFormat::S32;
    let mut interval = 0.2;
    let mut db_range = 90.0;
    let mut max_db = 0.0;
    let mut off_threshold = -60.0;
    let mut silence_duration = 10.0;
    let mut min_length = 600.0;
    let mut no_vumeter = false;

    let mut i = 1;
    let mut positional_args = Vec::new();

    while i < args.len() {
        match args[i].as_str() {
            "--list-targets" => {
                process::exit(list_targets());
            }
            "--source" | "--target" => {
                if i + 1 < args.len() {
                    source = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--rate" => {
                if i + 1 < args.len() {
                    rate = args[i + 1].parse().unwrap_or(96000);
                    i += 1;
                }
            }
            "--channels" => {
                if i + 1 < args.len() {
                    channels = args[i + 1].parse().unwrap_or(2);
                    i += 1;
                }
            }
            "--format" => {
                if i + 1 < args.len() {
                    format = SampleFormat::from_str(&args[i + 1]).unwrap_or(SampleFormat::S32);
                    i += 1;
                }
            }
            "--interval" => {
                if i + 1 < args.len() {
                    interval = args[i + 1].parse().unwrap_or(0.2);
                    i += 1;
                }
            }
            "--db-range" => {
                if i + 1 < args.len() {
                    db_range = args[i + 1].parse().unwrap_or(90.0);
                    i += 1;
                }
            }
            "--max-db" => {
                if i + 1 < args.len() {
                    max_db = args[i + 1].parse().unwrap_or(0.0);
                    i += 1;
                }
            }
            "--off-threshold" => {
                if i + 1 < args.len() {
                    off_threshold = args[i + 1].parse().unwrap_or(-60.0);
                    i += 1;
                }
            }
            "--silence-duration" => {
                if i + 1 < args.len() {
                    silence_duration = args[i + 1].parse().unwrap_or(10.0);
                    i += 1;
                }
            }
            "--min-length" => {
                if i + 1 < args.len() {
                    min_length = args[i + 1].parse().unwrap_or(600.0);
                    i += 1;
                }
            }
            "--no-vumeter" => {
                no_vumeter = true;
            }
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            arg if !arg.starts_with("--") => {
                positional_args.push(arg.to_string());
            }
            _ => {
                eprintln!("Unknown option: {}", args[i]);
                print_usage();
                process::exit(1);
            }
        }
        i += 1;
    }

    // Get filename from positional args
    if !positional_args.is_empty() {
        record_file = positional_args[0].clone();
    }

    // Determine the audio source address
    let source_address = if let Some(src) = source {
        src
    } else {
        // Try to auto-detect a PipeWire source
        let (selected_target, error_code) = validate_and_select_target(None, true);
        if error_code != 0 {
            process::exit(error_code);
        }
        format!("pipewire:{}", selected_target.unwrap())
    };

    // Parse the address to get backend and device
    let (backend, device) = match parse_audio_address(&source_address) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Error parsing audio source: {}", e);
            process::exit(1);
        }
    };

    println!("Using {} backend with device: {}", backend, device);

    // Create recorder
    let mut recorder = AudioRecorder::new(record_file.clone(), rate, channels, format, min_length);

    // Create audio stream
    let stream = match create_input_stream(&source_address, rate, channels, format) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to create audio stream: {}", e);
            process::exit(1);
        }
    };

    // Create VU meter
    let mut meter = VUMeter::new(
        stream,
        interval,
        db_range,
        max_db,
        off_threshold,
        silence_duration,
    );

    // Start recording
    if let Err(e) = meter.start() {
        eprintln!("Failed to start recording: {}", e);
        process::exit(1);
    }

    // Wait a moment for process to start
    thread::sleep(Duration::from_millis(100));

    println!("Recording started. Press Ctrl+C to stop.");
    println!("Waiting for signal...");
    println!();

    // Main loop
    loop {
        match process_audio_chunk(&mut meter) {
            Some(metrics) => {
                let any_channel_on = metrics.iter().any(|m| m.is_on);
                let any_clipping = metrics.iter().any(|m| m.has_clipped);

                // Get audio data for recording
                // Note: In a real implementation, we'd need to pass the actual audio samples
                // For now, we'll use a simplified approach
                let audio_data: Vec<Vec<i32>> = vec![vec![0]; channels];
                recorder.write_audio(&audio_data, any_channel_on, any_clipping);

                if !no_vumeter {
                    // Display VU meter with recording status
                    let rec_status = if recorder.is_recording() {
                        Some("[RECORDING]")
                    } else {
                        None
                    };
                    display_vu_meter(&metrics, db_range, max_db, rec_status).ok();
                }
            }
            None => {
                println!("\nRecording stopped.");
                break;
            }
        }
    }

    recorder.close();
}
