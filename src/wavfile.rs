//! WAV file I/O utilities for reading headers and audio data.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

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
