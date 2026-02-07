//! WAV file I/O utilities for reading headers and audio data.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

/// WAV file header information
#[derive(Debug)]
pub struct WavHeader {
    pub sample_rate: u32,
    pub num_channels: u16,
    pub bits_per_sample: u16,
    pub data_size: u32,
}

/// Read and parse a WAV file header.
///
/// # Arguments
/// * `file` - Buffered file reader positioned at the start of the WAV file
///
/// # Returns
/// Parsed WAV header information, or an error message
pub fn read_wav_header(file: &mut BufReader<File>) -> Result<WavHeader, String> {
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
/// Extract a segment from a WAV file and write it to a new WAV file
///
/// # Arguments
/// * `input_path` - Path to the input WAV file
/// * `output_path` - Path for the output WAV file
/// * `start_seconds` - Start time in seconds
/// * `duration_seconds` - Duration to extract in seconds
///
/// # Returns
/// Ok(()) on success, or an error message
pub fn extract_wav_segment(
    input_path: &str,
    output_path: &str,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<(), String> {
    // Open input file
    let input_file = File::open(input_path)
        .map_err(|e| format!("Failed to open input file: {}", e))?;
    let mut reader = BufReader::new(input_file);
    
    // Read header
    let header = read_wav_header(&mut reader)?;
    
    // Calculate byte positions
    let bytes_per_sample = (header.bits_per_sample / 8) as usize;
    let bytes_per_frame = bytes_per_sample * header.num_channels as usize;
    let start_frame = (start_seconds * header.sample_rate as f64) as usize;
    let duration_frames = (duration_seconds * header.sample_rate as f64) as usize;
    let start_byte = start_frame * bytes_per_frame;
    let segment_bytes = duration_frames * bytes_per_frame;
    
    // Seek to start position
    reader.seek(SeekFrom::Current(start_byte as i64))
        .map_err(|e| format!("Failed to seek to start position: {}", e))?;
    
    // Read segment data
    let mut segment_data = vec![0u8; segment_bytes];
    let bytes_read = reader.read(&mut segment_data)
        .map_err(|e| format!("Failed to read segment data: {}", e))?;
    segment_data.truncate(bytes_read);
    
    // Write output file
    let mut output_file = File::create(output_path)
        .map_err(|e| format!("Failed to create output file: {}", e))?;
    
    // Write WAV header
    write_wav_header(
        &mut output_file,
        segment_data.len(),
        header.sample_rate,
        header.num_channels,
        header.bits_per_sample,
    )?;
    
    // Write data
    output_file.write_all(&segment_data)
        .map_err(|e| format!("Failed to write segment data: {}", e))?;
    
    Ok(())
}

/// Write a WAV file header
fn write_wav_header(
    file: &mut File,
    data_size: usize,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> Result<(), String> {
    let byte_rate = sample_rate * channels as u32 * (bits_per_sample / 8) as u32;
    let block_align = channels * (bits_per_sample / 8);

    file.write_all(b"RIFF")
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&((data_size + 36) as u32).to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(b"WAVE")
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(b"fmt ")
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&16u32.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&1u16.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&channels.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&sample_rate.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&byte_rate.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&block_align.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&bits_per_sample.to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(b"data")
        .map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&(data_size as u32).to_le_bytes())
        .map_err(|e| format!("Write error: {}", e))?;

    Ok(())
}