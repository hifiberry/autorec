//! Strategy comparison tool - tests multiple detection strategies on a WAV file.

use autorec::detection_strategies::{
    absolute_threshold::AbsoluteThresholdDetector,
    relative_drop::RelativeDropDetector,
    energy_ratio::EnergyRatioDetector,
    transition::TransitionDetector,
    PauseDetectionStrategy,
};
use autorec::SampleFormat;
use std::env;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::process;

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
    
    if &buf[0..4] != b"RIFF" {
        return Err("Not a valid WAV file (missing RIFF header)".to_string());
    }
    
    if &buf[8..12] != b"WAVE" {
        return Err("Not a valid WAV file (missing WAVE marker)".to_string());
    }
    
    if &buf[12..16] != b"fmt " {
        return Err("Invalid WAV format chunk".to_string());
    }
    
    let num_channels = u16::from_le_bytes([buf[22], buf[23]]);
    let sample_rate = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let bits_per_sample = u16::from_le_bytes([buf[34], buf[35]]);
    
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
        
        file.seek(SeekFrom::Current(chunk_size as i64)).map_err(|e| format!("Seek error: {}", e))?;
    }
}

fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0) as u32;
    let secs = seconds % 60.0;
    format!("{:02}:{:05.2}", mins, secs)
}

struct StrategyResult {
    name: String,
    boundaries: Vec<f64>,
    song_count: u32,
}

fn test_strategy(
    file_path: &str,
    strategy: &mut dyn PauseDetectionStrategy,
    header: &WavHeader,
    chunk_size_ms: u32,
) -> StrategyResult {
    let file = File::open(file_path).unwrap();
    let mut reader = BufReader::new(file);
    read_wav_header(&mut reader).unwrap(); // Skip header
    
    let format = match header.bits_per_sample {
        16 => SampleFormat::S16,
        32 => SampleFormat::S32,
        _ => panic!("Unsupported bit depth"),
    };
    
    let bytes_per_sample = (header.bits_per_sample / 8) as usize;
    let chunk_samples = (header.sample_rate as f64 * chunk_size_ms as f64 / 1000.0) as usize;
    let chunk_bytes = chunk_samples * header.num_channels as usize * bytes_per_sample;
    
    let mut total_samples = 0usize;
    let mut boundaries = Vec::new();
    
    loop {
        let mut buffer = vec![0u8; chunk_bytes];
        let bytes_read = reader.read(&mut buffer).unwrap_or(0);
        
        if bytes_read == 0 {
            break;
        }
        
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
        
        if let Some(_) = strategy.feed_audio(&audio_data, format) {
            let timestamp_secs = total_samples as f64 / header.sample_rate as f64;
            boundaries.push(timestamp_secs);
        }
        
        total_samples += samples_in_chunk;
    }
    
    StrategyResult {
        name: strategy.name().to_string(),
        boundaries,
        song_count: strategy.song_number(),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        println!("Strategy Comparison Tool");
        println!("========================");
        println!();
        println!("Usage: strategy_compare <FILE.wav>");
        println!();
        println!("Tests multiple pause detection strategies and compares results.");
        process::exit(1);
    }
    
    let wav_file = &args[1];
    
    if !Path::new(wav_file).exists() {
        eprintln!("Error: File not found: {}", wav_file);
        process::exit(1);
    }
    
    println!("Strategy Comparison Tool");
    println!("========================");
    println!("File: {}", wav_file);
    println!();
    
    let file = File::open(wav_file).unwrap();
    let mut reader = BufReader::new(file);
    let header = read_wav_header(&mut reader).unwrap();
    
    println!("WAV Info:");
    println!("  Sample rate: {} Hz", header.sample_rate);
    println!("  Channels: {}", header.num_channels);
    println!("  Bits per sample: {}", header.bits_per_sample);
    let duration = header.data_size as f64 / (header.sample_rate as f64 * header.num_channels as f64 * (header.bits_per_sample / 8) as f64);
    println!("  Duration: {} ({:.2}s)", format_timestamp(duration), duration);
    println!();
    
    // Test different strategies
    println!("Testing strategies...");
    println!();
    
    let mut results = Vec::new();
    
    // Strategy 1: Absolute threshold -50dB
    println!("[1/9] Absolute Threshold: -50 dB, 200ms");
    let mut s1 = AbsoluteThresholdDetector::new(header.sample_rate, -50.0, 200);
    results.push(test_strategy(wav_file, &mut s1, &header, 200));
    
    // Strategy 2: Absolute threshold -40dB
    println!("[2/9] Absolute Threshold: -40 dB, 200ms");
    let mut s2 = AbsoluteThresholdDetector::new(header.sample_rate, -40.0, 200);
    results.push(test_strategy(wav_file, &mut s2, &header, 200));
    
    // Strategy 3: Relative drop 15dB
    println!("[3/9] Relative Drop: 15 dB below average, 200ms, 10s window");
    let mut s3 = RelativeDropDetector::new(header.sample_rate, 15.0, 200, 10.0);
    results.push(test_strategy(wav_file, &mut s3, &header, 200));
    
    // Strategy 4: Relative drop 20dB
    println!("[4/9] Relative Drop: 20 dB below average, 200ms, 10s window");
    let mut s4 = RelativeDropDetector::new(header.sample_rate, 20.0, 200, 10.0);
    results.push(test_strategy(wav_file, &mut s4, &header, 200));
    
    // Strategy 5: Energy ratio 1%
    println!("[5/9] Energy Ratio: 1% of max, 200ms, 10s window");
    let mut s5 = EnergyRatioDetector::new(header.sample_rate, 0.01, 200, 10.0);
    results.push(test_strategy(wav_file, &mut s5, &header, 200));
    
    // Strategy 6: Energy ratio 5%
    println!("[6/9] Energy Ratio: 5% of max, 200ms, 10s window");
    let mut s6 = EnergyRatioDetector::new(header.sample_rate, 0.05, 200, 10.0);
    results.push(test_strategy(wav_file, &mut s6, &header, 200));
    
    // Strategy 7: Transition detector (20% quietest, 10dB rise)
    println!("[7/9] Transition: 20th percentile quiet, 10dB rise, 500ms, 30s window");
    let mut s7 = TransitionDetector::new(header.sample_rate, 0.20, 10.0, 500, 30.0);
    results.push(test_strategy(wav_file, &mut s7, &header, 200));
    
    // Strategy 8: Transition detector (15% quietest, 8dB rise)
    println!("[8/9] Transition: 15th percentile quiet, 8dB rise, 500ms, 30s window");
    let mut s8 = TransitionDetector::new(header.sample_rate, 0.15, 8.0, 500, 30.0);
    results.push(test_strategy(wav_file, &mut s8, &header, 200));
    
    // Strategy 9: Transition detector (25% quietest, 12dB rise)
    println!("[9/9] Transition: 25th percentile quiet, 12dB rise, 1000ms, 30s window");
    let mut s9 = TransitionDetector::new(header.sample_rate, 0.25, 12.0, 1000, 30.0);
    results.push(test_strategy(wav_file, &mut s9, &header, 200));
    
    println!();
    println!("Results");
    println!("=======");
    println!();
    
    for result in &results {
        println!("{}:", result.name);
        println!("  Songs detected: {}", result.song_count);
        println!("  Boundaries: {}", result.boundaries.len());
        if !result.boundaries.is_empty() {
            println!("  Timestamps:");
            for (i, ts) in result.boundaries.iter().enumerate() {
                println!("    Song {} → {} at {}", i + 1, i + 2, format_timestamp(*ts));
            }
        }
        println!();
    }
    
    // Summary
    println!("Summary");
    println!("-------");
    for result in &results {
        println!("{:40} : {} songs, {} boundaries", 
                 result.name, result.song_count, result.boundaries.len());
    }
}
