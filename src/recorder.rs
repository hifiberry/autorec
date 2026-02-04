use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::vu_meter::SampleFormat;

#[derive(Debug)]
enum RecorderCommand {
    Start,
    Write(Vec<i32>),
    Stop,
}

#[allow(dead_code)]
pub struct AudioRecorder {
    base_filename: String,
    rate: u32,
    channels: usize,
    format: SampleFormat,
    min_length: f64,

    recording: Arc<Mutex<bool>>,
    current_file: Arc<Mutex<Option<String>>>,
    recording_start_time: Arc<Mutex<Option<Instant>>>,
    next_file_number: Arc<Mutex<usize>>,

    sender: Sender<RecorderCommand>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl AudioRecorder {
    pub fn new(
        base_filename: String,
        rate: u32,
        channels: usize,
        format: SampleFormat,
        min_length: f64,
    ) -> Self {
        // Initialize file counter by checking existing files
        let base_no_ext = if base_filename.ends_with(".wav") {
            base_filename.trim_end_matches(".wav").to_string()
        } else {
            base_filename.clone()
        };

        let mut n = 1;
        while Path::new(&format!("{}.{}.wav", base_no_ext, n)).exists() {
            n += 1;
        }

        let (sender, receiver) = channel();

        let recording = Arc::new(Mutex::new(false));
        let current_file = Arc::new(Mutex::new(None));
        let recording_start_time = Arc::new(Mutex::new(None));
        let next_file_number = Arc::new(Mutex::new(n));

        // Start recording thread
        let thread_handle = {
            let base_filename = base_filename.clone();
            let rate = rate;
            let channels = channels;
            let format = format;
            let min_length = min_length;
            let recording = Arc::clone(&recording);
            let current_file = Arc::clone(&current_file);
            let recording_start_time = Arc::clone(&recording_start_time);
            let next_file_number = Arc::clone(&next_file_number);

            thread::spawn(move || {
                Self::recording_worker(
                    receiver,
                    base_filename,
                    rate,
                    channels,
                    format,
                    min_length,
                    recording,
                    current_file,
                    recording_start_time,
                    next_file_number,
                );
            })
        };

        AudioRecorder {
            base_filename,
            rate,
            channels,
            format,
            min_length,
            recording,
            current_file,
            recording_start_time,
            next_file_number,
            sender,
            thread_handle: Some(thread_handle),
        }
    }

    fn get_next_filename(base_filename: &str, file_number: usize) -> String {
        let base_no_ext = if base_filename.ends_with(".wav") {
            base_filename.trim_end_matches(".wav")
        } else {
            base_filename
        };
        format!("{}.{}.wav", base_no_ext, file_number)
    }

    fn recording_worker(
        receiver: Receiver<RecorderCommand>,
        base_filename: String,
        rate: u32,
        channels: usize,
        format: SampleFormat,
        min_length: f64,
        recording: Arc<Mutex<bool>>,
        current_file: Arc<Mutex<Option<String>>>,
        recording_start_time: Arc<Mutex<Option<Instant>>>,
        next_file_number: Arc<Mutex<usize>>,
    ) {
        let mut wav_writer: Option<WavWriter> = None;

        while let Ok(command) = receiver.recv() {
            match command {
                RecorderCommand::Start => {
                    let is_recording = *recording.lock().unwrap();
                    if !is_recording {
                        let file_number = next_file_number.lock().unwrap();
                        let filename = Self::get_next_filename(&base_filename, *file_number);
                        drop(file_number);

                        match WavWriter::new(&filename, rate, channels, format) {
                            Ok(writer) => {
                                wav_writer = Some(writer);
                                *current_file.lock().unwrap() = Some(filename.clone());
                                *recording.lock().unwrap() = true;
                                *recording_start_time.lock().unwrap() = Some(Instant::now());
                                println!("\nStarted recording to {}", filename);
                            }
                            Err(e) => {
                                eprintln!("\nFailed to start recording: {}", e);
                            }
                        }
                    }
                }
                RecorderCommand::Write(samples) => {
                    if let Some(ref mut writer) = wav_writer {
                        if let Err(e) = writer.write_samples(&samples) {
                            eprintln!("\nError writing audio data: {}", e);
                        }
                    }
                }
                RecorderCommand::Stop => {
                    if let Some(mut writer) = wav_writer.take() {
                        if let Err(e) = writer.finalize() {
                            eprintln!("\nError finalizing WAV file: {}", e);
                        }

                        *recording.lock().unwrap() = false;

                        let duration = recording_start_time
                            .lock()
                            .unwrap()
                            .map(|t| t.elapsed().as_secs_f64())
                            .unwrap_or(0.0);

                        let filename = current_file.lock().unwrap().take().unwrap();

                        if duration < min_length {
                            println!(
                                "\nRecording too short ({:.1}s < {:.1}s), deleting {}",
                                duration, min_length, filename
                            );
                            if let Err(e) = std::fs::remove_file(&filename) {
                                eprintln!("\nError deleting file: {}", e);
                            }
                            // Don't increment file number since file was deleted
                        } else {
                            println!(
                                "\nStopped recording to {} (duration: {:.1}s)",
                                filename, duration
                            );
                            // Increment file number for next recording since this file was kept
                            let mut file_number = next_file_number.lock().unwrap();
                            *file_number += 1;
                        }

                        *recording_start_time.lock().unwrap() = None;
                    }
                }
            }
        }
    }

    pub fn write_audio(&self, audio_data: &[Vec<i32>], is_on: bool) {
        if is_on {
            let is_recording = *self.recording.lock().unwrap();
            if !is_recording {
                let _ = self.sender.send(RecorderCommand::Start);
            }

            // Interleave channels
            let mut interleaved = Vec::new();
            let frame_count = audio_data[0].len();
            for i in 0..frame_count {
                for ch in 0..self.channels {
                    if ch < audio_data.len() && i < audio_data[ch].len() {
                        interleaved.push(audio_data[ch][i]);
                    } else {
                        interleaved.push(0);
                    }
                }
            }

            let _ = self.sender.send(RecorderCommand::Write(interleaved));
        } else {
            let is_recording = *self.recording.lock().unwrap();
            if is_recording {
                let _ = self.sender.send(RecorderCommand::Stop);
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        *self.recording.lock().unwrap()
    }

    pub fn close(&mut self) {
        let is_recording = *self.recording.lock().unwrap();
        if is_recording {
            let _ = self.sender.send(RecorderCommand::Stop);
            // Give thread time to process stop command
            thread::sleep(Duration::from_millis(100));
        }

        // Take the handle first to avoid issues with sender being moved
        if let Some(handle) = self.thread_handle.take() {
            // Drop sender to close the channel and signal thread to exit
            drop(std::mem::replace(&mut self.sender, channel().0));
            let _ = handle.join();
        }
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.close();
    }
}

// Simple WAV file writer
struct WavWriter {
    file: File,
    data_size: usize,
    rate: u32,
    channels: usize,
    format: SampleFormat,
}

impl WavWriter {
    fn new(filename: &str, rate: u32, channels: usize, format: SampleFormat) -> io::Result<Self> {
        let mut file = File::create(filename)?;

        // Write WAV header (will be updated in finalize)
        let bits_per_sample = (format.bytes_per_sample() * 8) as u16;
        Self::write_wav_header(&mut file, 0, rate, channels as u16, bits_per_sample)?;

        Ok(WavWriter {
            file,
            data_size: 0,
            rate,
            channels,
            format,
        })
    }

    fn write_wav_header(
        file: &mut File,
        data_size: usize,
        rate: u32,
        channels: u16,
        bits_per_sample: u16,
    ) -> io::Result<()> {
        let byte_rate = rate * channels as u32 * (bits_per_sample / 8) as u32;
        let block_align = channels * (bits_per_sample / 8);

        file.write_all(b"RIFF")?;
        file.write_all(&((data_size + 36) as u32).to_le_bytes())?;
        file.write_all(b"WAVE")?;
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?; // fmt chunk size
        file.write_all(&1u16.to_le_bytes())?; // audio format (1 = PCM)
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&bits_per_sample.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&(data_size as u32).to_le_bytes())?;

        Ok(())
    }

    fn write_samples(&mut self, samples: &[i32]) -> io::Result<()> {
        match self.format {
            SampleFormat::S16 => {
                for &sample in samples {
                    let s16 = (sample as i16).to_le_bytes();
                    self.file.write_all(&s16)?;
                    self.data_size += 2;
                }
            }
            SampleFormat::S32 => {
                for &sample in samples {
                    let s32 = sample.to_le_bytes();
                    self.file.write_all(&s32)?;
                    self.data_size += 4;
                }
            }
        }
        Ok(())
    }

    fn finalize(&mut self) -> io::Result<()> {
        use std::io::Seek;

        // Update header with correct data size
        self.file.seek(io::SeekFrom::Start(0))?;
        let bits_per_sample = (self.format.bytes_per_sample() * 8) as u16;
        Self::write_wav_header(
            &mut self.file,
            self.data_size,
            self.rate,
            self.channels as u16,
            bits_per_sample,
        )?;
        self.file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_get_next_filename() {
        let filename = AudioRecorder::get_next_filename("test", 1);
        assert_eq!(filename, "test.1.wav");

        let filename = AudioRecorder::get_next_filename("test.wav", 5);
        assert_eq!(filename, "test.5.wav");

        let filename = AudioRecorder::get_next_filename("path/to/recording", 10);
        assert_eq!(filename, "path/to/recording.10.wav");
    }

    #[test]
    fn test_audio_recorder_creation() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_recording");
        let test_file_str = test_file.to_str().unwrap().to_string();

        let mut recorder = AudioRecorder::new(
            test_file_str.clone(),
            48000,
            2,
            SampleFormat::S32,
            1.0,
        );

        assert!(!recorder.is_recording());
        recorder.close();
    }

    #[test]
    fn test_recorder_state() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_recording_state");
        let test_file_str = test_file.to_str().unwrap().to_string();

        let mut recorder = AudioRecorder::new(
            test_file_str.clone(),
            48000,
            2,
            SampleFormat::S32,
            1.0,
        );

        // Initially not recording
        assert!(!recorder.is_recording());

        // Simulate audio with signal
        let audio_data = vec![vec![1000; 100], vec![1000; 100]];
        recorder.write_audio(&audio_data, true);

        // Give thread time to process
        std::thread::sleep(Duration::from_millis(100));

        // Should be recording now
        assert!(recorder.is_recording());

        // Stop and cleanup
        recorder.write_audio(&audio_data, false);
        std::thread::sleep(Duration::from_millis(100));
        recorder.close();

        // Cleanup any created files
        let _ = fs::remove_file(format!("{}.1.wav", test_file_str));
    }

    #[test]
    fn test_wav_header_generation() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_wav_header.wav");
        let test_file_str = test_file.to_str().unwrap();

        {
            let mut writer =
                WavWriter::new(test_file_str, 48000, 2, SampleFormat::S16).unwrap();

            // Write some samples
            let samples = vec![1000i32, -1000, 2000, -2000];
            writer.write_samples(&samples).unwrap();
            writer.finalize().unwrap();
        }

        // Read file and verify it exists and has content
        let metadata = fs::metadata(test_file_str).unwrap();
        assert!(metadata.len() > 44); // Should have header + data

        // Cleanup
        fs::remove_file(test_file_str).ok();
    }

    #[test]
    fn test_wav_writer_s16() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_s16.wav");
        let test_file_str = test_file.to_str().unwrap();

        {
            let mut writer =
                WavWriter::new(test_file_str, 44100, 1, SampleFormat::S16).unwrap();

            let samples = vec![0, 1000, -1000, 16000, -16000];
            writer.write_samples(&samples).unwrap();
            writer.finalize().unwrap();
        }

        let metadata = fs::metadata(test_file_str).unwrap();
        // Header (44 bytes) + 5 samples * 2 bytes = 54 bytes
        assert_eq!(metadata.len(), 54);

        fs::remove_file(test_file_str).ok();
    }

    #[test]
    fn test_wav_writer_s32() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_s32.wav");
        let test_file_str = test_file.to_str().unwrap();

        {
            let mut writer =
                WavWriter::new(test_file_str, 96000, 2, SampleFormat::S32).unwrap();

            let samples = vec![0, 100000, -100000, 1000000, -1000000];
            writer.write_samples(&samples).unwrap();
            writer.finalize().unwrap();
        }

        let metadata = fs::metadata(test_file_str).unwrap();
        // Header (44 bytes) + 5 samples * 4 bytes = 64 bytes
        assert_eq!(metadata.len(), 64);

        fs::remove_file(test_file_str).ok();
    }

    #[test]
    fn test_file_numbering() {
        let temp_dir = std::env::temp_dir();
        let test_base = temp_dir.join("test_numbering");
        let test_base_str = test_base.to_str().unwrap().to_string();

        // Create some existing files
        fs::write(format!("{}.1.wav", test_base_str), "dummy").ok();
        fs::write(format!("{}.2.wav", test_base_str), "dummy").ok();

        let mut recorder = AudioRecorder::new(
            test_base_str.clone(),
            48000,
            2,
            SampleFormat::S32,
            1.0,
        );

        // Next file number should be 3
        assert_eq!(*recorder.next_file_number.lock().unwrap(), 3);

        recorder.close();

        // Cleanup
        fs::remove_file(format!("{}.1.wav", test_base_str)).ok();
        fs::remove_file(format!("{}.2.wav", test_base_str)).ok();
    }
}

