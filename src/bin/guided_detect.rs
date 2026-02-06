//! Test guided detection using MusicBrainz metadata

use autorec::detection_strategies::guided::GuidedDetector;
use autorec::detection_strategies::PauseDetectionStrategy;
use autorec::musicbrainz::{fetch_release_info, parse_musicbrainz_url};
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
    
    if &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" || &buf[12..16] != b"fmt " {
        return Err("Not a valid WAV file".to_string());
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
            return Ok(WavHeader {
                sample_rate,
                num_channels,
                bits_per_sample,
                data_size: chunk_size,
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

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 3 {
        println!("Guided Detection Test");
        println!("=====================");
        println!();
        println!("Usage: guided_detect <FILE.wav> <MUSICBRAINZ_URL>");
        println!();
        println!("Example:");
        println!("  guided_detect recording.wav https://musicbrainz.org/release/768a1c5f-3657-4e29-aac4-c1de6ee5221f");
        println!();
        println!("Uses MusicBrainz track lengths to guide boundary detection.");
        process::exit(1);
    }
    
    let wav_file = &args[1];
    let mb_url = &args[2];
    
    if !Path::new(wav_file).exists() {
        eprintln!("Error: File not found: {}", wav_file);
        process::exit(1);
    }
    
    println!("Guided Detection Test");
    println!("=====================");
    println!("File: {}", wav_file);
    println!();
    
    // Parse MusicBrainz URL
    let release_id = parse_musicbrainz_url(mb_url).unwrap_or_else(|| {
        eprintln!("Error: Invalid MusicBrainz URL: {}", mb_url);
        process::exit(1);
    });
    
    println!("Fetching MusicBrainz data for release {}...", release_id);
    let all_tracks = fetch_release_info(&release_id).unwrap_or_else(|e| {
        eprintln!("Error fetching MusicBrainz data: {}", e);
        process::exit(1);
    });
    
    println!("Found {} tracks in release:", all_tracks.len());
    for track in &all_tracks {
        println!("  {}. {} - {:.1}s (starts @ {})", 
                 track.position, track.title, track.length_seconds, format_timestamp(track.expected_start));
    }
    println!();
    
    // Open WAV file
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
    
    // Match tracks to this file based on duration
    use autorec::musicbrainz::match_tracks_to_duration;
    let (track_offset, expected_tracks) = match_tracks_to_duration(&all_tracks, duration);
    
    println!("Matched {} tracks to this file:", expected_tracks.len());
    if track_offset == 0 {
        println!("  (Side A: tracks 1-{})", expected_tracks.len());
    } else {
        println!("  (Side B: tracks {}-{})", track_offset + 1, track_offset + expected_tracks.len());
    }
    for track in &expected_tracks {
        println!("  {}. {} - {:.1}s (starts @ {})", 
                 track.position, track.title, track.length_seconds, format_timestamp(track.expected_start));
    }
    println!();
    
    let format = match header.bits_per_sample {
        16 => SampleFormat::S16,
        32 => SampleFormat::S32,
        _ => {
            eprintln!("Error: Unsupported bit depth: {}", header.bits_per_sample);
            process::exit(1);
        }
    };
    
    // Create guided detector with 10-second search windows
    let mut detector = GuidedDetector::new(header.sample_rate, expected_tracks.clone(), 10.0);
    
    println!("Processing...");
    println!();
    
    let bytes_per_sample = (header.bits_per_sample / 8) as usize;
    let chunk_size_ms = 200;
    let chunk_samples = (header.sample_rate as f64 * chunk_size_ms as f64 / 1000.0) as usize;
    let chunk_bytes = chunk_samples * header.num_channels as usize * bytes_per_sample;
    
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
        
        if let Some(_) = detector.feed_audio(&audio_data, format) {
            let current_pos = (detector.get_debug_info().song_count - 1) as usize;
            if current_pos > 0 && current_pos <= expected_tracks.len() {
                boundaries.push(current_pos - 1);
            }
        }
    }
    
    println!();
    println!("Results");
    println!("=======");
    println!("Songs detected: {}", detector.song_number());
    println!("Boundaries found: {}", boundaries.len());
    println!();
    
    if !boundaries.is_empty() {
        println!("Detected Boundaries:");
        for idx in &boundaries {
            if *idx + 1 < expected_tracks.len() {
                let track = &expected_tracks[*idx];
                let next_track = &expected_tracks[*idx + 1];
                println!("  {} → {}", track.title, next_track.title);
                println!("    Expected: {}", format_timestamp(next_track.expected_start));
            }
        }
    }
}
