//! Pause analyzer tool - processes a WAV file and reports detected pauses.
//!
//! This tool is useful for training and adapting the pause detection algorithm.
//! It processes the entire file and outputs:
//! - Training phase information (noise floor detection)
//! - All detected song boundaries with timestamps
//! - Adaptive parameter changes
//! - Summary statistics

use autorec::{pause_detector::AdaptivePauseDetector, SampleFormat};
use std::env;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::process;

fn print_usage() {
    println!("Pause Analyzer - Detect song boundaries in a WAV file");
    println!();
    println!("Usage: pause_analyzer <FILE.wav> [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --chunk-size <MS>       Process chunk size in milliseconds (default: 200)");
    println!("  --verbose, -v           Show detailed RMS levels and detection state");
    println!("  --threshold <DB>        Override pause detection threshold (e.g. -40)");
    println!("  --pause-duration <MS>   Override minimum pause duration (e.g. 500)");
    println!("  --help                  Show this help message");
    println!();
    println!("Output:");
    println!("  - Training phase information");
    println!("  - Song boundary timestamps");
    println!("  - Adaptive parameter adjustments");
    println!("  - Summary statistics");
    println!();
    println!("Tuning tips:");
    println!("  - Use --verbose to see RMS levels during playback");
    println!("  - If no boundaries found: increase --threshold (less negative)");
    println!("  - If too many boundaries: decrease --threshold (more negative)");
    println!("  - Adjust --pause-duration for shorter/longer pause requirements");
}

#[derive(Debug)]
struct WavHeader {
    sample_rate: u32,
    num_channels: u16,
    bits_per_sample: u16,
    data_size: u32,
}

fn read_wav_header(file: &mut BufReader<File>) -> Result<WavHeader, String> {
    let mut buf = [0u8; 44];
    file.read_exact(&mut buf).map_err(|e| format!("Failed to read WAV header: {}", e))?;
    
    // Check RIFF header
    if &buf[0..4] != b"RIFF" {
        return Err("Not a valid WAV file (missing RIFF header)".to_string());
    }
    
    if &buf[8..12] != b"WAVE" {
        return Err("Not a valid WAV file (missing WAVE marker)".to_string());
    }
    
    // Parse format chunk
    if &buf[12..16] != b"fmt " {
        return Err("Invalid WAV format chunk".to_string());
    }
    
    let num_channels = u16::from_le_bytes([buf[22], buf[23]]);
    let sample_rate = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let bits_per_sample = u16::from_le_bytes([buf[34], buf[35]]);
    
    // Find data chunk (might not be at offset 36)
    file.seek(SeekFrom::Start(36)).map_err(|e| format!("Seek error: {}", e))?;
    
    loop {
        let mut chunk_header = [0u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            return Err("Could not find data chunk".to_string());
        }
        
        let chunk_size = u32::from_le_bytes([chunk_header[4], chunk_header[5], chunk_header[6], chunk_header[7]]);
        
        if &chunk_header[0..4] == b"data" {
            let data_size = chunk_size;
            return Ok(WavHeader {
                sample_rate,
                num_channels,
                bits_per_sample,
                data_size,
            });
        }
        
        // Skip this chunk
        file.seek(SeekFrom::Current(chunk_size as i64)).map_err(|e| format!("Seek error: {}", e))?;
    }
}

fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0) as u32;
    let secs = seconds % 60.0;
    format!("{:02}:{:05.2}", mins, secs)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }
    
    let mut wav_file: Option<String> = None;
    let mut chunk_size_ms: u32 = 200;
    let mut verbose = false;
    let mut override_threshold: Option<f32> = None;
    let mut override_pause_duration: Option<u32> = None;
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--chunk-size" => {
                if i + 1 < args.len() {
                    chunk_size_ms = args[i + 1].parse().unwrap_or(200);
                    i += 1;
                }
            }
            "--verbose" | "-v" => verbose = true,
            "--threshold" => {
                if i + 1 < args.len() {
                    override_threshold = args[i + 1].parse().ok();
                    i += 1;
                }
            }
            "--pause-duration" => {
                if i + 1 < args.len() {
                    override_pause_duration = args[i + 1].parse().ok();
                    i += 1;
                }
            }
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            arg => {
                if arg.starts_with("--") {
                    eprintln!("Unknown option: {}", arg);
                    process::exit(1);
                }
                if wav_file.is_none() {
                    wav_file = Some(arg.to_string());
                }
            }
        }
        i += 1;
    }
    
    let wav_file = wav_file.unwrap_or_else(|| {
        eprintln!("Error: No WAV file specified");
        print_usage();
        process::exit(1);
    });
    
    // Check if file exists
    if !Path::new(&wav_file).exists() {
        eprintln!("Error: File not found: {}", wav_file);
        process::exit(1);
    }
    
    println!("Pause Analyzer");
    println!("==============");
    println!("File: {}", wav_file);
    println!();
    
    // Open and parse WAV file
    let file = File::open(&wav_file).unwrap_or_else(|e| {
        eprintln!("Error opening file: {}", e);
        process::exit(1);
    });
    
    let mut reader = BufReader::new(file);
    let header = read_wav_header(&mut reader).unwrap_or_else(|e| {
        eprintln!("Error reading WAV header: {}", e);
        process::exit(1);
    });
    
    println!("WAV Info:");
    println!("  Sample rate: {} Hz", header.sample_rate);
    println!("  Channels: {}", header.num_channels);
    println!("  Bits per sample: {}", header.bits_per_sample);
    println!("  Duration: {:.2} seconds", header.data_size as f64 / (header.sample_rate as f64 * header.num_channels as f64 * (header.bits_per_sample / 8) as f64));
    println!();
    
    if let Some(thresh) = override_threshold {
        println!("Override threshold: {:.1} dB", thresh);
    }
    if let Some(dur) = override_pause_duration {
        println!("Override pause duration: {} ms", dur);
    }
    if verbose {
        println!("Verbose mode: ON");
    }
    if override_threshold.is_some() || override_pause_duration.is_some() || verbose {
        println!();
    }
    
    // Determine format
    let format = match header.bits_per_sample {
        16 => SampleFormat::S16,
        32 => SampleFormat::S32,
        _ => {
            eprintln!("Error: Unsupported bit depth: {}. Only 16 and 32 bit supported.", header.bits_per_sample);
            process::exit(1);
        }
    };
    
    let bytes_per_sample = (header.bits_per_sample / 8) as usize;
    let chunk_samples = (header.sample_rate as f64 * chunk_size_ms as f64 / 1000.0) as usize;
    let chunk_bytes = chunk_samples * header.num_channels as usize * bytes_per_sample;
    
    // Create pause detector
    let mut detector = AdaptivePauseDetector::new(header.sample_rate);
    
    // Apply overrides if provided
    if let Some(thresh) = override_threshold {
        detector.set_threshold_override(thresh);
    }
    if let Some(dur) = override_pause_duration {
        detector.set_pause_duration_override(dur);
    }
    
    println!("Processing with {}ms chunks...", chunk_size_ms);
    println!();
    
    let mut total_samples = 0usize;
    let mut song_boundaries = Vec::new();
    let mut is_training = true;
    let mut last_progress = 0;
    
    loop {
        let mut buffer = vec![0u8; chunk_bytes];
        let bytes_read = reader.read(&mut buffer).unwrap_or(0);
        
        if bytes_read == 0 {
            break;
        }
        
        // Convert bytes to samples
        let samples_in_chunk = bytes_read / (header.num_channels as usize * bytes_per_sample);
        let mut audio_data: Vec<Vec<i32>> = vec![Vec::with_capacity(samples_in_chunk); header.num_channels as usize];
        
        for i in 0..samples_in_chunk {
            for ch in 0..header.num_channels as usize {
                let byte_offset = (i * header.num_channels as usize + ch) * bytes_per_sample;
                if byte_offset + bytes_per_sample > bytes_read {
                    break;
                }
                
                let sample = match format {
                    SampleFormat::S16 => {
                        let s = i16::from_le_bytes([buffer[byte_offset], buffer[byte_offset + 1]]);
                        s as i32
                    }
                    SampleFormat::S32 => {
                        i32::from_le_bytes([
                            buffer[byte_offset],
                            buffer[byte_offset + 1],
                            buffer[byte_offset + 2],
                            buffer[byte_offset + 3],
                        ])
                    }
                };
                audio_data[ch].push(sample);
            }
        }
        
        // Feed to detector
        let event = detector.feed_audio(&audio_data, format);
        
        // Verbose output
        if verbose {
            let timestamp_secs = total_samples as f64 / header.sample_rate as f64;
            let progress_pct = (timestamp_secs / (header.data_size as f64 / (header.sample_rate as f64 * header.num_channels as f64 * (header.bits_per_sample / 8) as f64)) * 100.0) as u32;
            
            // Print progress every 5%
            if progress_pct > last_progress && progress_pct % 5 == 0 {
                let state_info = detector.get_debug_info();
                println!("[{:3}%] {} | RMS: {:6.1} dB | Thresh: {:6.1} dB | {}",
                        progress_pct,
                        format_timestamp(timestamp_secs),
                        state_info.current_rms_db,
                        state_info.threshold_db,
                        if state_info.in_pause { "IN PAUSE" } else { "        " });
                last_progress = progress_pct;
            }
        }
        
        // Check if training phase ended
        if is_training && detector.status_line().map(|s| !s.contains("Learning")).unwrap_or(false) {
            is_training = false;
            let timestamp_secs = total_samples as f64 / header.sample_rate as f64;
            println!("✓ Training complete at {}", format_timestamp(timestamp_secs));
            println!();
        }
        
        // Check for pause events
        if let Some(_) = event {
            let timestamp_secs = total_samples as f64 / header.sample_rate as f64;
            let song_num = detector.song_number();
            println!("🎵 Song boundary #{} detected at {}", song_num - 1, format_timestamp(timestamp_secs));
            song_boundaries.push((song_num - 1, timestamp_secs));
        }
        
        total_samples += samples_in_chunk;
    }
    
    println!();
    println!("Analysis Complete");
    println!("=================");
    
    let debug_info = detector.get_debug_info();
    println!("Final detection parameters:");
    println!("  Noise floor: {:.1} dB", debug_info.noise_floor_db);
    println!("  Pause threshold: {:.1} dB", debug_info.threshold_db);
    println!("  Pause duration: {} ms", debug_info.pause_duration_ms);
    println!();
    println!("Total duration: {}", format_timestamp(total_samples as f64 / header.sample_rate as f64));
    println!("Songs detected: {}", detector.song_number());
    println!("Boundaries found: {}", song_boundaries.len());
    
    if !song_boundaries.is_empty() {
        println!();
        println!("Song Boundaries:");
        for (song_num, timestamp) in &song_boundaries {
            println!("  Song {} -> {} at {}", song_num, song_num + 1, format_timestamp(*timestamp));
        }
        
        // Calculate song lengths
        println!();
        println!("Song Durations:");
        let mut prev_time = 0.0;
        for (song_num, timestamp) in &song_boundaries {
            let duration = timestamp - prev_time;
            println!("  Song {}: {:.1}s ({})", song_num, duration, format_timestamp(duration));
            prev_time = *timestamp;
        }
        
        // Last song
        let last_duration = (total_samples as f64 / header.sample_rate as f64) - prev_time;
        println!("  Song {}: {:.1}s ({}) [incomplete - end of file]", 
                 detector.song_number(), last_duration, format_timestamp(last_duration));
        
        // Statistics
        let total_time = total_samples as f64 / header.sample_rate as f64;
        let avg_song_length = total_time / detector.song_number() as f64;
        println!();
        println!("Statistics:");
        println!("  Average song length: {:.1}s ({})", avg_song_length, format_timestamp(avg_song_length));
    }
}
