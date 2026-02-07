use autorec::{create_input_stream, display_vu_meter, list_targets, parse_audio_address, process_audio_chunk, validate_and_select_target, AudioRecorder, Config, SampleFormat, VUMeter};
use std::env;
use std::process;
use std::thread;
use std::time::Duration;
use crossterm::{
    event::{poll, read, Event, KeyCode, KeyEvent},
    terminal::{disable_raw_mode, enable_raw_mode},
};

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
    println!("  --show-defaults          Show default configuration values and exit");
    println!("  --show-saved-defaults    Show saved default configuration from file and exit");
    println!("  --save-defaults          Save current command-line options as defaults");
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
    println!("  --duration <SEC>         Maximum recording duration in seconds (0=unlimited)");
    println!("  --detect-interval <SEC>  Song detection interval in seconds (default: 180, 0=off)");
    println!("  --no-shazam              Disable song detection");
    println!("  --no-vumeter             Disable VU meter display (simple text output)");
    println!("  --no-keyboard            Disable keyboard shortcuts (no raw mode)");
    println!("  --no-generate-cue        Disable automatic CUE file generation after recording");
    println!("  --help                   Show this help message");
    println!();
    println!("Configuration:");
    println!("  Defaults can be saved to ~/.state/autorec/defaults.toml using --save-defaults.");
    println!("  Saved defaults override built-in defaults, and command-line options override both.");
    println!();
    println!("Examples:");
    println!("  record vinyl --source pipewire:riaa.monitor");
    println!("  record tape --source alsa:hw:1,0 --rate 48000");
    println!("  record test --source /path/to/source.flac");
    println!("  record --source alsa:hw:1,0 --rate 48000 --save-defaults  # Save as defaults");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Load saved defaults from config file if available
    let saved_config = Config::load().unwrap_or_else(|_| Config::new());

    // Built-in default values
    let builtin_defaults = Config {
        source: None,
        rate: Some(96000),
        channels: Some(2),
        format: Some("s32".to_string()),
        interval: Some(0.2),
        db_range: Some(90.0),
        max_db: Some(0.0),
        off_threshold: Some(-60.0),
        silence_duration: Some(10.0),
        min_length: Some(600.0),
        no_vumeter: Some(false),
        no_keyboard: Some(false),
    };

    // Start with built-in defaults, then apply saved config
    let mut effective_config = builtin_defaults.clone();
    effective_config.merge(&saved_config);

    // Current values (will be updated by command-line args)
    let mut record_file = "recording".to_string();
    let mut source: Option<String> = effective_config.source.clone();
    let mut rate = effective_config.rate.unwrap_or(96000);
    let mut channels = effective_config.channels.unwrap_or(2);
    let mut format = SampleFormat::from_str(&effective_config.format.clone().unwrap_or_else(|| "s32".to_string()))
        .unwrap_or(SampleFormat::S32);
    let mut interval = effective_config.interval.unwrap_or(0.2);
    let mut db_range = effective_config.db_range.unwrap_or(90.0);
    let mut max_db = effective_config.max_db.unwrap_or(0.0);
    let mut off_threshold = effective_config.off_threshold.unwrap_or(-60.0);
    let mut silence_duration = effective_config.silence_duration.unwrap_or(10.0);
    let mut min_length = effective_config.min_length.unwrap_or(600.0);
    let mut no_vumeter = effective_config.no_vumeter.unwrap_or(false);
    let mut no_keyboard = effective_config.no_keyboard.unwrap_or(false);
    let mut duration: Option<f64> = None;
    let mut generate_cue = true;  // Generate CUE files by default

    // Track which options were explicitly set on command line
    let mut cmdline_config = Config::new();
    let mut save_defaults = false;

    let mut i = 1;
    let mut positional_args = Vec::new();

    while i < args.len() {
        match args[i].as_str() {
            "--list-targets" => {
                process::exit(list_targets());
            }
            "--show-defaults" => {
                println!("Built-in default settings:");
                println!();
                
                // Auto-detect the default source
                let (selected_target, error_code) = validate_and_select_target(None, false);
                let source_info = if error_code == 0 && selected_target.is_some() {
                    format!("pipewire:{}", selected_target.unwrap())
                } else {
                    "No PipeWire source available".to_string()
                };
                
                println!("  Audio source:       {} (auto-detected)", source_info);
                println!("  Sample rate:        96000 Hz");
                println!("  Channels:           2");
                println!("  Format:             s32");
                println!("  Update interval:    0.2 seconds");
                println!("  dB range:           90 dB");
                println!("  Maximum dB:         0 dB");
                println!("  Off threshold:      -60 dB");
                println!("  Silence duration:   10 seconds");
                println!("  Min recording:      600 seconds (10 minutes)");
                println!("  VU meter:           enabled");
                println!("  Keyboard shortcuts: enabled");
                process::exit(0);
            }
            "--show-saved-defaults" => {
                if let Ok(config_path) = Config::get_config_path() {
                    if config_path.exists() {
                        println!("Saved defaults from {:?}:", config_path);
                        println!();
                        saved_config.print("Configuration");
                    } else {
                        println!("No saved defaults file found at {:?}", config_path);
                        println!("Use --save-defaults to create one.");
                    }
                } else {
                    println!("Could not determine config file path");
                }
                process::exit(0);
            }
            "--save-defaults" => {
                save_defaults = true;
            }
            "--source" | "--target" => {
                if i + 1 < args.len() {
                    source = Some(args[i + 1].clone());
                    cmdline_config.source = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--rate" => {
                if i + 1 < args.len() {
                    rate = args[i + 1].parse().unwrap_or(96000);
                    cmdline_config.rate = Some(rate);
                    i += 1;
                }
            }
            "--channels" => {
                if i + 1 < args.len() {
                    channels = args[i + 1].parse().unwrap_or(2);
                    cmdline_config.channels = Some(channels);
                    i += 1;
                }
            }
            "--format" => {
                if i + 1 < args.len() {
                    format = SampleFormat::from_str(&args[i + 1]).unwrap_or(SampleFormat::S32);
                    cmdline_config.format = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--interval" => {
                if i + 1 < args.len() {
                    interval = args[i + 1].parse().unwrap_or(0.2);
                    cmdline_config.interval = Some(interval);
                    i += 1;
                }
            }
            "--db-range" => {
                if i + 1 < args.len() {
                    db_range = args[i + 1].parse().unwrap_or(90.0);
                    cmdline_config.db_range = Some(db_range);
                    i += 1;
                }
            }
            "--max-db" => {
                if i + 1 < args.len() {
                    max_db = args[i + 1].parse().unwrap_or(0.0);
                    cmdline_config.max_db = Some(max_db);
                    i += 1;
                }
            }
            "--off-threshold" => {
                if i + 1 < args.len() {
                    off_threshold = args[i + 1].parse().unwrap_or(-60.0);
                    cmdline_config.off_threshold = Some(off_threshold);
                    i += 1;
                }
            }
            "--silence-duration" => {
                if i + 1 < args.len() {
                    silence_duration = args[i + 1].parse().unwrap_or(10.0);
                    cmdline_config.silence_duration = Some(silence_duration);
                    i += 1;
                }
            }
            "--min-length" => {
                if i + 1 < args.len() {
                    min_length = args[i + 1].parse().unwrap_or(600.0);
                    cmdline_config.min_length = Some(min_length);
                    i += 1;
                }
            }
            "--no-vumeter" => {
                no_vumeter = true;
                cmdline_config.no_vumeter = Some(true);
            }
            "--no-keyboard" => {
                no_keyboard = true;
                cmdline_config.no_keyboard = Some(true);
            }
            "--generate-cue" => generate_cue = true,
            "--no-generate-cue" => generate_cue = false,
            "--duration" => {
                if i + 1 < args.len() {
                    let dur_value: f64 = args[i + 1].parse().unwrap_or(60.0);
                    // duration=0 means unlimited (same as not setting it)
                    if dur_value > 0.0 {
                        duration = Some(dur_value);
                    } else {
                        duration = None;
                    }
                    if dur_value > 0.0 {
                        min_length = 0.0;  // Disable min length check when using duration
                    }
                    i += 1;
                }
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

    // Save defaults if requested
    if save_defaults {
        // Merge command-line config with saved config
        let mut config_to_save = saved_config.clone();
        config_to_save.merge(&cmdline_config);
        
        match config_to_save.save() {
            Ok(_) => {
                if let Ok(config_path) = Config::get_config_path() {
                    println!("Defaults saved to {:?}", config_path);
                    println!();
                    config_to_save.print("Saved configuration");
                }
                process::exit(0);
            }
            Err(e) => {
                eprintln!("Error saving defaults: {}", e);
                process::exit(1);
            }
        }
    }

    // Get filename from positional args
    if !positional_args.is_empty() {
        record_file = positional_args[0].clone();
    }

    // Determine the audio source address
    let source_address = if let Some(src) = source {
        // Parse to determine backend
        let (backend, device) = match parse_audio_address(&src) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Error parsing audio source '{}': {}", src, e);
                process::exit(1);
            }
        };
        
        // Validate PipeWire sources exist
        if backend == "pipewire" {
            let (validated_target, error_code) = validate_and_select_target(Some(&device), true);
            if error_code != 0 {
                eprintln!("\nAvailable sources:");
                list_targets();
                process::exit(error_code);
            }
            format!("pipewire:{}", validated_target.unwrap())
        } else {
            src
        }
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

    if no_keyboard {
        println!("Recording started. Press Ctrl+C to stop.");
    } else {
        println!("Recording started. Press ESC or 'q' to quit.");
        // Enable raw mode for keyboard input
        enable_raw_mode().ok();
    }
    println!("Waiting for signal...");
    println!();

    // Track start time for duration limit
    let start_time = std::time::Instant::now();

    // Main loop
    loop {
        // Check for keyboard input (non-blocking) if keyboard mode is enabled
        if !no_keyboard && poll(Duration::from_millis(0)).unwrap_or(false) {
            if let Ok(Event::Key(KeyEvent { code, .. })) = read() {
                match code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                        disable_raw_mode().ok();
                        println!("\nExiting...");
                        break;
                    }
                    _ => {}
                }
            }
        }

        // Check if duration limit has been reached
        if let Some(max_duration) = duration {
            let elapsed = start_time.elapsed().as_secs_f64();
            if elapsed >= max_duration {
                if !no_keyboard {
                    disable_raw_mode().ok();
                }
                println!("\nDuration limit reached. Exiting...");
                break;
            }
        }

        // Read and process audio data once
        match process_audio_chunk(&mut meter) {
            Some((metrics, audio_data)) => {
                let any_channel_on = metrics.iter().any(|m| m.is_on);
                let is_recording = recorder.is_recording();

                // Write the actual audio data to the recorder
                recorder.write_audio(&audio_data, any_channel_on);

                if !no_vumeter {
                    // Build status lines
                    let mut status_parts: Vec<String> = Vec::new();

                    // Recording status
                    if is_recording {
                        if let Some(filename) = recorder.current_filename() {
                            status_parts.push(format!("[RECORDING to {}]", filename));
                        } else {
                            status_parts.push("[RECORDING]".to_string());
                        }
                    }

                    let rec_status = if status_parts.is_empty() {
                        None
                    } else {
                        Some(status_parts.join("  "))
                    };
                    display_vu_meter(&metrics, db_range, max_db, rec_status.as_deref()).ok();
                }
            }
            None => {
                if !no_keyboard {
                    disable_raw_mode().ok();
                }
                println!("\nRecording stopped.");
                break;
            }
        }
    }

    // Generate CUE files if requested
    if generate_cue {
        let recorded_files = recorder.get_recorded_files();
        if !recorded_files.is_empty() {
            println!("\nGenerating CUE files for {} recording(s)...", recorded_files.len());
            for file in &recorded_files {
                println!("  Processing: {}", file);
                let output = process::Command::new("cue_creator")
                    .arg(file)
                    .output();
                
                match output {
                    Ok(result) if result.status.success() => {
                        println!("    ✓ CUE file generated");
                    }
                    Ok(result) => {
                        eprintln!("    ✗ Failed to generate CUE file");
                        if !result.stderr.is_empty() {
                            eprintln!("      {}", String::from_utf8_lossy(&result.stderr));
                        }
                    }
                    Err(e) => {
                        eprintln!("    ✗ Error running cue_creator: {}", e);
                    }
                }
            }
        } else {
            println!("\nNo recordings were created, skipping CUE generation.");
        }
    }

    recorder.close();
}