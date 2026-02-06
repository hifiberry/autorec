use crate::vu_meter::SampleFormat;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use pipewire as pw;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::pod::Pod;

/// Parse an audio source address in the format "backend:device"
/// Examples: "pipewire:input1", "pwpipe:input1", "alsa:hw:0,0", "file:/path/to/audio.wav"
/// If no backend is specified, tries to auto-detect
pub fn parse_audio_address(address: &str) -> Result<(String, String), String> {
    // First check for ALSA-style addresses without explicit backend
    if address.starts_with("hw:") || address.starts_with("plughw:") || address == "default" {
        return Ok(("alsa".to_string(), address.to_string()));
    }
    
    // Look for backend prefix
    if let Some(colon_pos) = address.find(':') {
        let backend = &address[..colon_pos];
        let device = &address[colon_pos + 1..];
        
        match backend.to_lowercase().as_str() {
            "pipewire" | "pw" => Ok(("pipewire".to_string(), device.to_string())),
            "pwpipe" => Ok(("pwpipe".to_string(), device.to_string())),
            "alsa" => Ok(("alsa".to_string(), device.to_string())),
            "file" => Ok(("file".to_string(), device.to_string())),
            _ => {
                // Unknown backend, default to PipeWire for compatibility
                Ok(("pipewire".to_string(), address.to_string()))
            }
        }
    } else {
        // No colon - check for file path or extension indicators
        if address.contains('/') || address.ends_with(".wav") || address.ends_with(".mp3") || 
           address.ends_with(".flac") || address.ends_with(".WAV") || address.ends_with(".MP3") || 
           address.ends_with(".FLAC") {
            return Ok(("file".to_string(), address.to_string()));
        }
        
        // Default to PipeWire
        Ok(("pipewire".to_string(), address.to_string()))
    }
}

/// Create an audio input stream from an address string
pub fn create_input_stream(
    address: &str,
    rate: u32,
    channels: usize,
    format: SampleFormat,
) -> Result<Box<dyn AudioInputStream>, String> {
    let (backend, device) = parse_audio_address(address)?;
    
    match backend.as_str() {
        "pipewire" => Ok(Box::new(PipeWireInputStream::new(
            device, rate, channels, format,
        )?)),
        "pwpipe" => Ok(Box::new(PwPipeInputStream::new(
            device, rate, channels, format,
        ))),
        "alsa" => Ok(Box::new(AlsaInputStream::new(
            device, rate, channels, format,
        ))),
        "file" => FileInputStream::new(device, rate, channels, format)
            .map(|s| Box::new(s) as Box<dyn AudioInputStream>),
        _ => Err(format!("Unsupported backend: {}", backend)),
    }
}

/// Base trait for audio streams with common properties
pub trait AudioStream {
    /// Get the sample rate in Hz
    fn sample_rate(&self) -> u32;
    
    /// Get the number of channels
    fn channels(&self) -> usize;
    
    /// Get the sample format
    fn sample_format(&self) -> SampleFormat;
    
    /// Get bytes per sample based on format
    fn bytes_per_sample(&self) -> usize {
        self.sample_format().bytes_per_sample()
    }
    
    /// Get bytes per frame (all channels)
    fn bytes_per_frame(&self) -> usize {
        self.channels() * self.bytes_per_sample()
    }
}

/// Trait for audio input streams that can read audio data
pub trait AudioInputStream: AudioStream {
    /// Read a chunk of audio data
    /// Returns a vector of channels, where each channel is a vector of samples
    fn read_chunk(&mut self, frames: usize) -> Option<Vec<Vec<i32>>>;
    
    /// Start the audio input stream
    fn start(&mut self) -> Result<(), String>;
    
    /// Stop the audio input stream
    fn stop(&mut self);
    
    /// Check if the stream is active
    fn is_active(&self) -> bool;
}

/// Native PipeWire audio input stream using the Rust pipewire crate
pub struct PipeWireInputStream {
    _target: String,
    rate: u32,
    channels: usize,
    format: SampleFormat,
    active: bool,
    buffer: Arc<Mutex<Vec<Vec<i32>>>>,
    thread_handle: Option<JoinHandle<()>>,
    quit_flag: Arc<AtomicBool>,
}

impl PipeWireInputStream {
    /// Create a new native PipeWire input stream
    pub fn new(target: String, rate: u32, channels: usize, format: SampleFormat) -> Result<Self, String> {
        Ok(PipeWireInputStream {
            _target: target,
            rate,
            channels,
            format,
            active: false,
            buffer: Arc::new(Mutex::new(Vec::new())),
            thread_handle: None,
            quit_flag: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl AudioStream for PipeWireInputStream {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    
    fn channels(&self) -> usize {
        self.channels
    }
    
    fn sample_format(&self) -> SampleFormat {
        self.format
    }
}

impl AudioInputStream for PipeWireInputStream {
    fn read_chunk(&mut self, frames: usize) -> Option<Vec<Vec<i32>>> {
        if !self.active {
            return None;
        }
        
        // Wait for enough data in the buffer (with timeout)
        let max_waits = 50; // Wait up to 500ms
        for _ in 0..max_waits {
            let buffer = self.buffer.lock().unwrap();
            if !buffer.is_empty() && buffer[0].len() >= frames {
                break;
            }
            drop(buffer);
            std::thread::sleep(Duration::from_millis(10));
        }
        
        // Check if we have enough data in the buffer
        let mut buffer = self.buffer.lock().unwrap();
        
        if buffer.is_empty() || buffer[0].len() < frames {
            return None;
        }
        
        // Extract the requested frames
        let mut result = Vec::with_capacity(self.channels);
        for ch in 0..self.channels {
            let samples: Vec<i32> = buffer[ch].drain(..frames).collect();
            result.push(samples);
        }
        
        Some(result)
    }
    
    fn start(&mut self) -> Result<(), String> {
        if self.active {
            return Ok(());
        }
        
        let buffer = self.buffer.clone();
        let rate = self.rate;
        let channels = self.channels;
        let format = self.format;
        
        // Reset quit flag
        self.quit_flag.store(false, Ordering::Relaxed);
        let _quit_flag_thread = self.quit_flag.clone();
        
        // Spawn thread to run PipeWire mainloop
        let thread_handle = thread::spawn(move || {
            // Initialize PipeWire in this thread
            pw::init();
            
            let main_loop = match pw::main_loop::MainLoop::new(None) {
                Ok(ml) => ml,
                Err(e) => {
                    eprintln!("Failed to create main loop: {:?}", e);
                    return;
                }
            };
            
            let context = match pw::context::Context::new(&main_loop) {
                Ok(ctx) => ctx,
                Err(e) => {
                    eprintln!("Failed to create context: {:?}", e);
                    return;
                }
            };
            
            let core = match context.connect(None) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to connect to PipeWire: {:?}", e);
                    return;
                }
            };
            
            // Create audio format info
            let audio_format = match format {
                SampleFormat::S16 => AudioFormat::S16LE,
                SampleFormat::S32 => AudioFormat::S32LE,
            };
            
            let mut audio_info = AudioInfoRaw::new();
            audio_info.set_format(audio_format);
            audio_info.set_rate(rate);
            audio_info.set_channels(channels as u32);
            
            // Create the stream
            let stream = match pw::stream::Stream::new(
                &core,
                "autorec-capture",
                pw::properties::properties! {
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Capture",
                    *pw::keys::MEDIA_ROLE => "Music",
                },
            ) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to create stream: {:?}", e);
                    return;
                }
            };
            
            // Set up stream listener
            let _listener = stream
                .add_local_listener_with_user_data(())
                .process(move |stream, _user_data| {
                    if let Some(mut buffer_data) = stream.dequeue_buffer() {
                        let datas = buffer_data.datas_mut();
                        if let Some(data) = datas.first_mut() {
                            let chunk = data.chunk();
                            let size = chunk.size() as usize;
                            
                            if let Some(samples_slice) = data.data() {
                                // Convert to samples per channel
                                let bytes_per_sample = format.bytes_per_sample();
                                let frame_size = bytes_per_sample * channels;
                                let num_frames = size / frame_size;
                                
                                let mut channel_samples: Vec<Vec<i32>> = vec![Vec::new(); channels];
                                
                                for frame in 0..num_frames {
                                    for ch in 0..channels {
                                        let offset = frame * frame_size + ch * bytes_per_sample;
                                        let sample = match format {
                                            SampleFormat::S16 => {
                                                if offset + 2 <= samples_slice.len() {
                                                    i16::from_le_bytes([samples_slice[offset], samples_slice[offset + 1]]) as i32
                                                } else {
                                                    0
                                                }
                                            }
                                            SampleFormat::S32 => {
                                                if offset + 4 <= samples_slice.len() {
                                                    i32::from_le_bytes([
                                                        samples_slice[offset],
                                                        samples_slice[offset + 1],
                                                        samples_slice[offset + 2],
                                                        samples_slice[offset + 3],
                                                    ])
                                                } else {
                                                    0
                                                }
                                            }
                                        };
                                        channel_samples[ch].push(sample);
                                    }
                                }
                                
                                // Append to buffer
                                let mut buf = buffer.lock().unwrap();
                                if buf.is_empty() {
                                    *buf = channel_samples;
                                } else {
                                    for (ch, samples) in channel_samples.into_iter().enumerate() {
                                        buf[ch].extend(samples);
                                    }
                                }
                            }
                        }
                    }
                })
                .register();
            
            if _listener.is_err() {
                eprintln!("Failed to register listener");
                return;
            }
            
            // Build parameters
            let obj = pw::spa::pod::Object {
                type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                id: pw::spa::param::ParamType::EnumFormat.as_raw(),
                properties: audio_info.into(),
            };
            let values: Vec<u8> = match pw::spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &pw::spa::pod::Value::Object(obj),
            ) {
                Ok((cursor, _)) => cursor.into_inner(),
                Err(e) => {
                    eprintln!("Failed to serialize audio info: {:?}", e);
                    return;
                }
            };
            
            let mut params = [Pod::from_bytes(&values).unwrap()];
            
            // Connect the stream
            if let Err(e) = stream.connect(
                pw::spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT
                    | pw::stream::StreamFlags::MAP_BUFFERS
                    | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            ) {
                eprintln!("Failed to connect stream: {:?}", e);
                return;
            }
            
            // Run the main loop (blocks until quit() is called)
            // Note: Currently we cannot gracefully stop the mainloop because:
            // 1. iterate() doesn't properly trigger audio callbacks
            // 2. MainLoop uses Rc and cannot be shared across threads
            // The thread will be killed when the process exits
            main_loop.run();
        });
        
        self.thread_handle = Some(thread_handle);
        self.active = true;
        
        // Give the stream a moment to start up
        std::thread::sleep(Duration::from_millis(200));
        
        Ok(())
    }
    
    fn stop(&mut self) {
        self.active = false;
        
        // Signal the thread to quit
        self.quit_flag.store(true, Ordering::Relaxed);
        
        // Wait for thread to finish
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        
        self.buffer.lock().unwrap().clear();
    }
    
    fn is_active(&self) -> bool {
        self.active
    }
}

/// PipeWire-based audio input stream using pw-record subprocess (legacy)
pub struct PwPipeInputStream {
    target: String,
    rate: u32,
    channels: usize,
    format: SampleFormat,
    process: Option<Child>,
}

impl PwPipeInputStream {
    /// Create a new PipeWire subprocess input stream
    pub fn new(target: String, rate: u32, channels: usize, format: SampleFormat) -> Self {
        PwPipeInputStream {
            target,
            rate,
            channels,
            format,
            process: None,
        }
    }
}

impl AudioStream for PwPipeInputStream {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    
    fn channels(&self) -> usize {
        self.channels
    }
    
    fn sample_format(&self) -> SampleFormat {
        self.format
    }
}

impl AudioInputStream for PwPipeInputStream {
    fn read_chunk(&mut self, frames: usize) -> Option<Vec<Vec<i32>>> {
        let chunk_size = frames * self.bytes_per_frame();
        let format = self.format;
        let channels = self.channels;
        
        let process = self.process.as_mut()?;
        let stdout = process.stdout.as_mut()?;
        let mut buffer = vec![0u8; chunk_size];
        
        if stdout.read_exact(&mut buffer).is_err() {
            return None;
        }
        
        // Convert bytes to samples
        let samples: Vec<i32> = match format {
            SampleFormat::S16 => buffer
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as i32)
                .collect(),
            SampleFormat::S32 => buffer
                .chunks_exact(4)
                .map(|chunk| i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect(),
        };
        
        // Reshape into channels
        let mut audio = vec![Vec::new(); channels];
        for (i, sample) in samples.iter().enumerate() {
            audio[i % channels].push(*sample);
        }
        
        Some(audio)
    }
    
    fn start(&mut self) -> Result<(), String> {
        let process = Command::new("pw-record")
            .arg("--target")
            .arg(&self.target)
            .arg("--rate")
            .arg(self.rate.to_string())
            .arg("--channels")
            .arg(self.channels.to_string())
            .arg("--format")
            .arg(self.format.as_str())
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start pw-record: {}", e))?;
        
        self.process = Some(process);
        Ok(())
    }
    
    fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
    
    fn is_active(&self) -> bool {
        self.process.is_some()
    }
}

impl Drop for PwPipeInputStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// ALSA-based audio input stream using arecord
pub struct AlsaInputStream {
    device: String,
    rate: u32,
    channels: usize,
    format: SampleFormat,
    process: Option<Child>,
}

impl AlsaInputStream {
    /// Create a new ALSA input stream
    pub fn new(device: String, rate: u32, channels: usize, format: SampleFormat) -> Self {
        AlsaInputStream {
            device,
            rate,
            channels,
            format,
            process: None,
        }
    }
}

impl AudioStream for AlsaInputStream {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    
    fn channels(&self) -> usize {
        self.channels
    }
    
    fn sample_format(&self) -> SampleFormat {
        self.format
    }
}

impl AudioInputStream for AlsaInputStream {
    fn read_chunk(&mut self, frames: usize) -> Option<Vec<Vec<i32>>> {
        let chunk_size = frames * self.bytes_per_frame();
        let format = self.format;
        let channels = self.channels;
        
        let process = self.process.as_mut()?;
        let stdout = process.stdout.as_mut()?;
        let mut buffer = vec![0u8; chunk_size];
        
        if stdout.read_exact(&mut buffer).is_err() {
            return None;
        }
        
        // Convert bytes to samples
        let samples: Vec<i32> = match format {
            SampleFormat::S16 => buffer
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as i32)
                .collect(),
            SampleFormat::S32 => buffer
                .chunks_exact(4)
                .map(|chunk| i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect(),
        };
        
        // Reshape into channels
        let mut audio = vec![Vec::new(); channels];
        for (i, sample) in samples.iter().enumerate() {
            audio[i % channels].push(*sample);
        }
        
        Some(audio)
    }
    
    fn start(&mut self) -> Result<(), String> {
        // Format the ALSA format string
        let alsa_format = match self.format {
            SampleFormat::S16 => "S16_LE",
            SampleFormat::S32 => "S32_LE",
        };
        
        let process = Command::new("arecord")
            .arg("-D")
            .arg(&self.device)
            .arg("-r")
            .arg(self.rate.to_string())
            .arg("-c")
            .arg(self.channels.to_string())
            .arg("-f")
            .arg(alsa_format)
            .arg("-t")
            .arg("raw")
            .arg("--")  // Read from stdin, output to stdout
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start arecord: {}", e))?;
        
        self.process = Some(process);
        Ok(())
    }
    
    fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
    
    fn is_active(&self) -> bool {
        self.process.is_some()
    }
}

impl Drop for AlsaInputStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// File-based audio input stream for WAV, MP3, and FLAC files
/// Maintains correct timing by controlling playback speed
pub struct FileInputStream {
    file_path: String,
    rate: u32,
    channels: usize,
    format: SampleFormat,
    format_reader: Option<Box<dyn FormatReader>>,
    decoder: Option<Box<dyn Decoder>>,
    track_id: Option<u32>,
    active: bool,
    start_time: Option<Instant>,
    frames_read: u64,
    buffer: Vec<Vec<i32>>,  // Buffered samples organized by channel
}

impl FileInputStream {
    /// Create a new file input stream
    pub fn new(file_path: String, rate: u32, channels: usize, format: SampleFormat) -> Result<Self, String> {
        // Verify file exists
        if !Path::new(&file_path).exists() {
            return Err(format!("File not found: {}", file_path));
        }
        
        Ok(FileInputStream {
            file_path,
            rate,
            channels,
            format,
            format_reader: None,
            decoder: None,
            track_id: None,
            active: false,
            start_time: None,
            frames_read: 0,
            buffer: Vec::new(),
        })
    }
    
    /// Refill the internal buffer by decoding more audio
    fn refill_buffer(&mut self) -> Result<(), String> {
        // Read the next packet
        let packet = {
            let format_reader = self.format_reader.as_mut()
                .ok_or("Format reader not initialized")?;
            match format_reader.next_packet() {
                Ok(packet) => packet,
                Err(_) => {
                    // End of stream - loop back to the beginning
                    let _ = format_reader; // Release the borrow
                    self.stop();
                    self.start()?;
                    return Ok(());
                }
            }
        };
        
        // Decode the packet and extract sample data immediately
        let (num_channels, channel_data) = {
            let decoder = self.decoder.as_mut()
                .ok_or("Decoder not initialized")?;
            let decoded = decoder.decode(&packet)
                .map_err(|e| format!("Decode error: {}", e))?;
            
            // Extract data from AudioBufferRef before it goes out of scope
            extract_audio_samples(&decoded, self.channels)
        };
        
        // Now append to our buffer with no borrowing conflicts
        if self.buffer.is_empty() {
            self.buffer = vec![Vec::new(); self.channels];
        }
        
        for (ch, data) in channel_data.into_iter().enumerate().take(self.channels) {
            self.buffer[ch].extend(data);
        }
        
        // If file has fewer channels than requested, duplicate the last channel
        if num_channels < self.channels {
            for ch in num_channels..self.channels {
                let last_data = self.buffer[num_channels - 1].clone();
                self.buffer[ch].extend(last_data);
            }
        }
        
        Ok(())
    }
}

/// Extract audio samples from an AudioBufferRef into vectors of i32 samples per channel
/// Returns (num_channels_in_source, channel_data)
fn extract_audio_samples(audio_buf: &AudioBufferRef, max_channels: usize) -> (usize, Vec<Vec<i32>>) {
    let spec = audio_buf.spec();
    let num_source_channels = spec.channels.count();
    let mut channel_data: Vec<Vec<i32>> = vec![Vec::new(); max_channels.min(num_source_channels)];
    
    // Convert based on the audio buffer type
    match audio_buf {
        AudioBufferRef::U8(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| ((s as i32 - 128) << 24))
                );
            }
        }
        AudioBufferRef::U16(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| ((s as i32 - 32768) << 16))
                );
            }
        }
        AudioBufferRef::U24(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| ((s.inner() as i32) << 8))
                );
            }
        }
        AudioBufferRef::U32(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| s.wrapping_sub(0x80000000) as i32)
                );
            }
        }
        AudioBufferRef::S8(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| (s as i32) << 24)
                );
            }
        }
        AudioBufferRef::S16(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| (s as i32) << 16)
                );
            }
        }
        AudioBufferRef::S24(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| s.inner() << 8)
                );
            }
        }
        AudioBufferRef::S32(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| s)
                );
            }
        }
        AudioBufferRef::F32(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| (s.clamp(-1.0, 1.0) * 2147483647.0) as i32)
                );
            }
        }
        AudioBufferRef::F64(buf) => {
            for ch in 0..max_channels.min(num_source_channels) {
                let samples = buf.chan(ch);
                channel_data[ch].extend(
                    samples.iter()
                        .map(|&s| (s.clamp(-1.0, 1.0) * 2147483647.0) as i32)
                );
            }
        }
    }
    
    (num_source_channels, channel_data)
}

impl AudioStream for FileInputStream {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    
    fn channels(&self) -> usize {
        self.channels
    }
    
    fn sample_format(&self) -> SampleFormat {
        self.format
    }
}

impl AudioInputStream for FileInputStream {
    fn read_chunk(&mut self, frames: usize) -> Option<Vec<Vec<i32>>> {
        if !self.active {
            return None;
        }
        
        // Ensure we have enough data in the buffer
        while self.buffer.is_empty() || self.buffer[0].len() < frames {
            if let Err(_) = self.refill_buffer() {
                return None;
            }
        }
        
        // Calculate timing to maintain correct playback speed
        if let Some(start_time) = self.start_time {
            let expected_time = Duration::from_secs_f64(
                self.frames_read as f64 / self.rate as f64
            );
            let elapsed = start_time.elapsed();
            
            if elapsed < expected_time {
                // Sleep to maintain correct timing
                std::thread::sleep(expected_time - elapsed);
            }
        }
        
        // Extract the requested number of frames
        let mut result = Vec::with_capacity(self.channels);
        for ch in 0..self.channels {
            let samples: Vec<i32> = self.buffer[ch].drain(..frames).collect();
            result.push(samples);
        }
        
        self.frames_read += frames as u64;
        Some(result)
    }
    
    fn start(&mut self) -> Result<(), String> {
        if self.active {
            return Ok(());
        }
        
        // Open the file
        let file = File::open(&self.file_path)
            .map_err(|e| format!("Failed to open file: {}", e))?;
        
        // Create a media source stream
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        
        // Create a hint to help identify the format
        let mut hint = Hint::new();
        if let Some(ext) = Path::new(&self.file_path).extension() {
            hint.with_extension(ext.to_str().unwrap_or(""));
        }
        
        // Probe the media source
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| format!("Failed to probe file: {}", e))?;
        
        let format_reader = probed.format;
        
        // Find the first audio track
        let track = format_reader.tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .ok_or("No audio tracks found")?;
        
        let track_id = track.id;
        
        // Get the actual sample rate from the file (we'll use our requested rate for output)
        let _file_rate = track.codec_params.sample_rate
            .ok_or("Sample rate not specified in file")?;
        
        // Create a decoder
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("Failed to create decoder: {}", e))?;
        
        self.format_reader = Some(format_reader);
        self.decoder = Some(decoder);
        self.track_id = Some(track_id);
        self.active = true;
        self.start_time = Some(Instant::now());
        self.frames_read = 0;
        self.buffer.clear();
        
        Ok(())
    }
    
    fn stop(&mut self) {
        self.active = false;
        self.format_reader = None;
        self.decoder = None;
        self.track_id = None;
        self.start_time = None;
        self.frames_read = 0;
        self.buffer.clear();
    }
    
    fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for FileInputStream {
    fn drop(&mut self) {
        self.stop();
    }
}

// Implement AudioInputStream for Box<dyn AudioInputStream> to allow dynamic dispatch
impl AudioStream for Box<dyn AudioInputStream> {
    fn sample_rate(&self) -> u32 {
        (**self).sample_rate()
    }
    
    fn channels(&self) -> usize {
        (**self).channels()
    }
    
    fn sample_format(&self) -> SampleFormat {
        (**self).sample_format()
    }
}

impl AudioInputStream for Box<dyn AudioInputStream> {
    fn read_chunk(&mut self, frames: usize) -> Option<Vec<Vec<i32>>> {
        (**self).read_chunk(frames)
    }
    
    fn start(&mut self) -> Result<(), String> {
        (**self).start()
    }
    
    fn stop(&mut self) {
        (**self).stop()
    }
    
    fn is_active(&self) -> bool {
        (**self).is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipewire_stream_creation() {
        let stream = PipeWireInputStream::new(
            "test_target".to_string(),
            48000,
            2,
            SampleFormat::S32,
        ).expect("Failed to create PipeWireInputStream");
        
        assert_eq!(stream.sample_rate(), 48000);
        assert_eq!(stream.channels(), 2);
        assert_eq!(stream.bytes_per_sample(), 4);
        assert_eq!(stream.bytes_per_frame(), 8); // 2 channels * 4 bytes
        assert!(!stream.is_active());
    }

    #[test]
    fn test_stream_properties() {
        let stream = PipeWireInputStream::new(
            "test".to_string(),
            96000,
            4,
            SampleFormat::S16,
        ).expect("Failed to create PipeWireInputStream");
        
        assert_eq!(stream.sample_rate(), 96000);
        assert_eq!(stream.channels(), 4);
        assert_eq!(stream.bytes_per_sample(), 2);
        assert_eq!(stream.bytes_per_frame(), 8); // 4 channels * 2 bytes
    }

    #[test]
    fn test_sample_format_consistency() {
        let stream_s16 = PipeWireInputStream::new(
            "test".to_string(),
            44100,
            2,
            SampleFormat::S16,
        ).expect("Failed to create PipeWireInputStream");
        assert!(matches!(stream_s16.sample_format(), SampleFormat::S16));
        
        let stream_s32 = PipeWireInputStream::new(
            "test".to_string(),
            44100,
            2,
            SampleFormat::S32,
        ).expect("Failed to create PipeWireInputStream");
        assert!(matches!(stream_s32.sample_format(), SampleFormat::S32));
    }

    #[test]
    fn test_alsa_stream_creation() {
        let stream = AlsaInputStream::new(
            "hw:0,0".to_string(),
            48000,
            2,
            SampleFormat::S32,
        );
        
        assert_eq!(stream.sample_rate(), 48000);
        assert_eq!(stream.channels(), 2);
        assert_eq!(stream.bytes_per_sample(), 4);
        assert_eq!(stream.bytes_per_frame(), 8);
        assert!(!stream.is_active());
    }

    #[test]
    fn test_parse_audio_address_pipewire() {
        let (backend, device) = parse_audio_address("pipewire:input1").unwrap();
        assert_eq!(backend, "pipewire");
        assert_eq!(device, "input1");
        
        let (backend, device) = parse_audio_address("pw:test.monitor").unwrap();
        assert_eq!(backend, "pipewire");
        assert_eq!(device, "test.monitor");
    }

    #[test]
    fn test_parse_audio_address_alsa() {
        let (backend, device) = parse_audio_address("alsa:hw:0,0").unwrap();
        assert_eq!(backend, "alsa");
        assert_eq!(device, "hw:0,0");
        
        let (backend, device) = parse_audio_address("alsa:default").unwrap();
        assert_eq!(backend, "alsa");
        assert_eq!(device, "default");
    }

    #[test]
    fn test_parse_audio_address_auto_detect() {
        // ALSA-style addresses should auto-detect as ALSA
        let (backend, device) = parse_audio_address("hw:0,0").unwrap();
        assert_eq!(backend, "alsa");
        assert_eq!(device, "hw:0,0");
        
        let (backend, device) = parse_audio_address("plughw:1,0").unwrap();
        assert_eq!(backend, "alsa");
        assert_eq!(device, "plughw:1,0");
        
        let (backend, device) = parse_audio_address("default").unwrap();
        assert_eq!(backend, "alsa");
        assert_eq!(device, "default");
        
        // Other formats default to PipeWire
        let (backend, device) = parse_audio_address("input.monitor").unwrap();
        assert_eq!(backend, "pipewire");
        assert_eq!(device, "input.monitor");
    }

    #[test]
    fn test_parse_audio_address_invalid() {
        // Unknown backends now default to pipewire for compatibility
        let (backend, device) = parse_audio_address("unknown:device").unwrap();
        assert_eq!(backend, "pipewire");
        assert_eq!(device, "unknown:device");
    }
    
    #[test]
    fn test_parse_audio_address_file() {
        // Test file path detection
        let (backend, device) = parse_audio_address("/path/to/audio.wav").unwrap();
        assert_eq!(backend, "file");
        assert_eq!(device, "/path/to/audio.wav");
        
        let (backend, device) = parse_audio_address("./test.mp3").unwrap();
        assert_eq!(backend, "file");
        assert_eq!(device, "./test.mp3");
        
        let (backend, device) = parse_audio_address("file:/tmp/music.flac").unwrap();
        assert_eq!(backend, "file");
        assert_eq!(device, "/tmp/music.flac");  // The colon separates backend from path
        
        // Test file extension detection
        let (backend, device) = parse_audio_address("audio.WAV").unwrap();
        assert_eq!(backend, "file");
        assert_eq!(device, "audio.WAV");
    }

    #[test]
    fn test_create_input_stream() {
        // Test creating PipeWire stream
        let stream = create_input_stream(
            "pipewire:test",
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        assert_eq!(stream.sample_rate(), 48000);
        assert_eq!(stream.channels(), 2);
        
        // Test creating ALSA stream
        let stream = create_input_stream(
            "alsa:hw:0,0",
            44100,
            2,
            SampleFormat::S16,
        ).unwrap();
        assert_eq!(stream.sample_rate(), 44100);
        assert_eq!(stream.channels(), 2);
        
        // Test auto-detection
        let stream = create_input_stream(
            "hw:0,0",
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        assert_eq!(stream.sample_rate(), 48000);
    }
    
    // Helper function to create test audio files
    fn create_test_audio_file(path: &str, format: &str, duration_secs: f64, sample_rate: u32, freq: f64) -> Result<(), String> {
        use std::process::Command;
        
        // Generate a sine wave using sox
        let output = Command::new("sox")
            .arg("-n")
            .arg("-r")
            .arg(sample_rate.to_string())
            .arg("-c")
            .arg("2")
            .arg(path)
            .arg("synth")
            .arg(duration_secs.to_string())
            .arg("sine")
            .arg(freq.to_string())
            .output()
            .map_err(|e| format!("Failed to run sox: {}", e))?;
        
        if !output.status.success() {
            return Err(format!("sox failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        
        // If not WAV, convert using ffmpeg
        if format != "wav" {
            let temp_wav = format!("{}.temp.wav", path);
            std::fs::rename(path, &temp_wav).map_err(|e| format!("Failed to rename: {}", e))?;
            
            let output = Command::new("ffmpeg")
                .arg("-i")
                .arg(&temp_wav)
                .arg("-y")
                .arg(path)
                .output()
                .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;
            
            std::fs::remove_file(&temp_wav).ok();
            
            if !output.status.success() {
                return Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr)));
            }
        }
        
        Ok(())
    }
    
    #[test]
    fn test_file_input_stream_wav() {
        use std::fs;
        use std::time::Instant;
        
        // Create a temporary WAV file - 1 second of audio
        let test_file = "/tmp/test_autorec_file.wav";
        if let Err(e) = create_test_audio_file(test_file, "wav", 1.0, 48000, 440.0) {
            eprintln!("Skipping test_file_input_stream_wav: {}", e);
            return;
        }
        
        // Create file input stream
        let mut stream = FileInputStream::new(
            test_file.to_string(),
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        
        assert_eq!(stream.sample_rate(), 48000);
        assert_eq!(stream.channels(), 2);
        assert!(!stream.is_active());
        
        // Start the stream
        stream.start().unwrap();
        assert!(stream.is_active());
        
        // Read chunks and verify timing
        let chunk_frames = 4800; // 0.1 seconds worth
        let start = Instant::now();
        let mut total_frames = 0;
        
        for _ in 0..5 {
            if let Some(chunk) = stream.read_chunk(chunk_frames) {
                assert_eq!(chunk.len(), 2); // 2 channels
                assert_eq!(chunk[0].len(), chunk_frames);
                assert_eq!(chunk[1].len(), chunk_frames);
                total_frames += chunk_frames;
            }
        }
        
        let elapsed = start.elapsed();
        let expected_duration = std::time::Duration::from_secs_f64(total_frames as f64 / 48000.0);
        
        // Timing should be close (within 100ms tolerance)
        let timing_diff = elapsed.as_secs_f64() - expected_duration.as_secs_f64();
        assert!(timing_diff.abs() < 0.1, 
            "Timing error too large: expected {:.3}s, got {:.3}s (diff: {:.3}s)", 
            expected_duration.as_secs_f64(), elapsed.as_secs_f64(), timing_diff);
        
        stream.stop();
        assert!(!stream.is_active());
        
        // Cleanup
        fs::remove_file(test_file).ok();
    }
    
    #[test]
    fn test_file_input_stream_mp3() {
        use std::fs;
        
        // Create a temporary MP3 file - 0.5 seconds
        let test_file = "/tmp/test_autorec_file.mp3";
        if let Err(e) = create_test_audio_file(test_file, "mp3", 0.5, 44100, 880.0) {
            eprintln!("Skipping test_file_input_stream_mp3: {}", e);
            return;
        }
        
        // Create file input stream
        let mut stream = FileInputStream::new(
            test_file.to_string(),
            44100,
            2,
            SampleFormat::S16,
        ).unwrap();
        
        assert_eq!(stream.sample_rate(), 44100);
        assert_eq!(stream.channels(), 2);
        
        // Start and read some data
        stream.start().unwrap();
        
        let chunk = stream.read_chunk(4410).unwrap(); // 0.1 seconds
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk[0].len(), 4410);
        
        stream.stop();
        
        // Cleanup
        fs::remove_file(test_file).ok();
    }
    
    #[test]
    fn test_file_input_stream_flac() {
        use std::fs;
        
        // Create a temporary FLAC file - 0.5 seconds
        let test_file = "/tmp/test_autorec_file.flac";
        if let Err(e) = create_test_audio_file(test_file, "flac", 0.5, 48000, 1000.0) {
            eprintln!("Skipping test_file_input_stream_flac: {}", e);
            return;
        }
        
        // Create file input stream
        let mut stream = FileInputStream::new(
            test_file.to_string(),
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        
        assert_eq!(stream.sample_rate(), 48000);
        assert_eq!(stream.channels(), 2);
        
        // Start and read some data
        stream.start().unwrap();
        
        let chunk = stream.read_chunk(4800).unwrap(); // 0.1 seconds
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk[0].len(), 4800);
        
        // Verify we got actual audio data (not all zeros)
        let max_sample = chunk[0].iter().map(|&s| s.abs()).max().unwrap();
        assert!(max_sample > 0, "Expected non-zero audio samples");
        
        stream.stop();
        
        // Cleanup
        fs::remove_file(test_file).ok();
    }
    
    #[test]
    fn test_file_input_stream_timing() {
        use std::fs;
        use std::time::Instant;
        
        // Create a test file - 2 seconds
        let test_file = "/tmp/test_autorec_timing.wav";
        if let Err(e) = create_test_audio_file(test_file, "wav", 2.0, 48000, 440.0) {
            eprintln!("Skipping test_file_input_stream_timing: {}", e);
            return;
        }
        
        let mut stream = FileInputStream::new(
            test_file.to_string(),
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        
        stream.start().unwrap();
        
        // Read 1 second of audio in 10 chunks
        let chunk_frames = 4800; // 0.1 seconds each
        let start = Instant::now();
        
        for _ in 0..10 {
            stream.read_chunk(chunk_frames);
        }
        
        let elapsed = start.elapsed();
        
        // Should take approximately 1 second (±150ms tolerance for system variance)
        assert!(elapsed.as_secs_f64() >= 0.85 && elapsed.as_secs_f64() <= 1.15,
            "Expected ~1.0s playback time, got {:.3}s", elapsed.as_secs_f64());
        
        stream.stop();
        fs::remove_file(test_file).ok();
    }
    
    #[test]
    fn test_file_input_stream_nonexistent() {
        let result = FileInputStream::new(
            "/nonexistent/file.wav".to_string(),
            48000,
            2,
            SampleFormat::S32,
        );
        
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.contains("File not found"));
        }
    }
    
    #[test]
    fn test_file_input_stream_create_via_address() {
        use std::fs;
        
        // Create a test file
        let test_file = "/tmp/test_autorec_address.wav";
        if let Err(e) = create_test_audio_file(test_file, "wav", 0.5, 48000, 440.0) {
            eprintln!("Skipping test_file_input_stream_create_via_address: {}", e);
            return;
        }
        
        // Test creating via create_input_stream
        let stream = create_input_stream(
            test_file,
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        
        assert_eq!(stream.sample_rate(), 48000);
        assert_eq!(stream.channels(), 2);
        
        // Also test with file: prefix
        let stream = create_input_stream(
            &format!("file:{}", test_file),
            48000,
            2,
            SampleFormat::S32,
        ).unwrap();
        
        assert_eq!(stream.sample_rate(), 48000);
        
        fs::remove_file(test_file).ok();
    }
}

/// Discover available audio sources for each backend
pub mod discovery {
    use crate::pipewire_utils;
    use std::process::Command;
    
    #[derive(Debug, Clone)]
    pub struct AudioSource {
        pub backend: String,
        pub url: String,
        pub description: Option<String>,
    }
    
    /// Discover PipeWire sources
    pub fn discover_pipewire_sources() -> Vec<AudioSource> {
        pipewire_utils::get_available_targets()
            .into_iter()
            .map(|src| AudioSource {
                backend: "pipewire".to_string(),
                url: format!("pipewire:{}", src.name),
                description: src.description,
            })
            .collect()
    }
    
    /// Discover PwPipe sources (same as PipeWire)
    pub fn discover_pwpipe_sources() -> Vec<AudioSource> {
        pipewire_utils::get_available_targets()
            .into_iter()
            .map(|src| AudioSource {
                backend: "pwpipe".to_string(),
                url: format!("pwpipe:{}", src.name),
                description: src.description,
            })
            .collect()
    }
    
    /// Discover ALSA sources
    pub fn discover_alsa_sources() -> Vec<AudioSource> {
        let mut sources = Vec::new();
        
        // Try to list ALSA devices using arecord
        if let Ok(output) = Command::new("arecord")
            .arg("-l")
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                
                for line in stdout.lines() {
                    if line.starts_with("card") {
                        // Parse lines like: "card 0: PCH [HDA Intel PCH], device 0: ALC269VC Analog [ALC269VC Analog]"
                        if let Some(card_start) = line.find("card ") {
                            if let Some(colon) = line[card_start..].find(':') {
                                let card_part = &line[card_start + 5..card_start + colon];
                                if let Ok(card_num) = card_part.parse::<u32>() {
                                    if let Some(device_pos) = line.find("device ") {
                                        if let Some(device_colon) = line[device_pos..].find(':') {
                                            let device_part = &line[device_pos + 7..device_pos + device_colon];
                                            if let Ok(device_num) = device_part.parse::<u32>() {
                                                let hw_addr = format!("hw:{},{}", card_num, device_num);
                                                
                                                // Extract description (text in square brackets)
                                                let desc = if let Some(desc_start) = line.rfind('[') {
                                                    if let Some(desc_end) = line[desc_start..].find(']') {
                                                        Some(line[desc_start + 1..desc_start + desc_end].to_string())
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    None
                                                };
                                                
                                                sources.push(AudioSource {
                                                    backend: "alsa".to_string(),
                                                    url: format!("alsa:{}", hw_addr),
                                                    description: desc,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Always add the "default" device
        if !sources.iter().any(|s| s.url == "alsa:default") {
            sources.insert(0, AudioSource {
                backend: "alsa".to_string(),
                url: "alsa:default".to_string(),
                description: Some("Default ALSA device".to_string()),
            });
        }
        
        sources
    }
    
    /// Discover audio files in the current directory
    pub fn discover_file_sources() -> Vec<AudioSource> {
        use std::fs;
        
        let mut sources = Vec::new();
        
        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        if let Some(path_str) = entry.path().to_str() {
                            let path_lower = path_str.to_lowercase();
                            if path_lower.ends_with(".wav") 
                                || path_lower.ends_with(".mp3") 
                                || path_lower.ends_with(".flac") {
                                sources.push(AudioSource {
                                    backend: "file".to_string(),
                                    url: format!("file:{}", path_str),
                                    description: Some(format!("Audio file: {}", entry.file_name().to_string_lossy())),
                                });
                            }
                        }
                    }
                }
            }
        }
        
        // Sort by filename
        sources.sort_by(|a, b| a.url.cmp(&b.url));
        
        sources
    }
    
    /// Discover all available audio sources from all backends
    pub fn discover_all_sources() -> Vec<AudioSource> {
        let mut all_sources = Vec::new();
        
        all_sources.extend(discover_pipewire_sources());
        all_sources.extend(discover_alsa_sources());
        all_sources.extend(discover_file_sources());
        
        all_sources
    }
}
