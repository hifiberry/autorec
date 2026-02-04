use crate::audio_stream::AudioInputStream;
use crate::decibel;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub enum SampleFormat {
    S16,
    S32,
}

impl SampleFormat {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "s16" | "s16le" => Ok(SampleFormat::S16),
            "s32" | "s32le" => Ok(SampleFormat::S32),
            _ => Err(format!("Unsupported format: {}", s)),
        }
    }

    pub fn bytes_per_sample(&self) -> usize {
        match self {
            SampleFormat::S16 => 2,
            SampleFormat::S32 => 4,
        }
    }

    pub fn max_value(&self) -> f64 {
        match self {
            SampleFormat::S16 => 32768.0,
            SampleFormat::S32 => 2147483648.0,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            SampleFormat::S16 => "s16",
            SampleFormat::S32 => "s32",
        }
    }
}

pub struct VUMeter<S: AudioInputStream> {
    pub stream: S,
    pub update_interval: f64,
    pub db_range: f64,
    pub max_db: f64,
    pub min_db: f64,
    pub off_threshold: f64,
    pub silence_duration: f64,

    frames_per_update: usize,
    history_size: usize,
    db_history: Vec<VecDeque<f64>>,
    clip_history: Vec<VecDeque<bool>>,
    peak_history: Vec<VecDeque<f64>>,
}

impl<S: AudioInputStream> VUMeter<S> {
    pub fn new(
        stream: S,
        update_interval: f64,
        db_range: f64,
        max_db: f64,
        off_threshold: f64,
        silence_duration: f64,
    ) -> Self {
        let min_db = max_db - db_range;
        let rate = stream.sample_rate();
        let channels = stream.channels();
        let frames_per_update = (rate as f64 * update_interval) as usize;
        let history_size = (silence_duration / update_interval) as usize;

        let db_history = vec![VecDeque::new(); channels];
        let clip_history = vec![VecDeque::new(); channels];
        let peak_history = vec![VecDeque::new(); channels];

        VUMeter {
            stream,
            update_interval,
            db_range,
            max_db,
            min_db,
            off_threshold,
            silence_duration,
            frames_per_update,
            history_size,
            db_history,
            clip_history,
            peak_history,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.stream.start()
    }

    pub fn stop(&mut self) {
        self.stream.stop()
    }

    pub fn read_audio_chunk(&mut self) -> Option<Vec<Vec<i32>>> {
        self.stream.read_chunk(self.frames_per_update)
    }

    pub fn calculate_db(&self, audio_channel: &[i32]) -> f64 {
        decibel::calculate_rms_db(
            audio_channel,
            self.stream.sample_format().max_value(),
            self.min_db,
            self.max_db,
        )
    }

    pub fn calculate_peak_db(&self, audio_channel: &[i32]) -> f64 {
        decibel::calculate_peak_db(
            audio_channel,
            self.stream.sample_format().max_value(),
            self.min_db,
            self.max_db,
        )
    }

    pub fn detect_clipping(&self, audio_channel: &[i32]) -> bool {
        let threshold = decibel::clipping_threshold(
            self.stream.sample_format().max_value(),
            0.999,
        );
        decibel::detect_clipping(audio_channel, threshold)
    }

    pub fn update_history(
        &mut self,
        channel: usize,
        db_value: f64,
        peak_db_value: f64,
        is_clipping: bool,
    ) -> (f64, f64, bool, bool) {
        let channels = self.stream.channels();
        if channel >= channels {
            return (self.min_db, self.min_db, false, false);
        }

        self.db_history[channel].push_back(db_value);
        self.peak_history[channel].push_back(peak_db_value);
        self.clip_history[channel].push_back(is_clipping);

        // Keep only last N values
        if self.db_history[channel].len() > self.history_size {
            self.db_history[channel].pop_front();
        }
        if self.peak_history[channel].len() > self.history_size {
            self.peak_history[channel].pop_front();
        }
        if self.clip_history[channel].len() > self.history_size {
            self.clip_history[channel].pop_front();
        }

        let max_db = self.db_history[channel]
            .iter()
            .copied()
            .fold(self.min_db, f64::max);
        let max_peak_db = self.peak_history[channel]
            .iter()
            .copied()
            .fold(self.min_db, f64::max);
        let is_on = self.db_history[channel]
            .iter()
            .any(|&db| db > self.off_threshold);
        let has_clipped = self.clip_history[channel].iter().any(|&c| c);

        (max_db, max_peak_db, is_on, has_clipped)
    }

    pub fn is_any_channel_on(&self) -> bool {
        for ch_history in &self.db_history {
            if ch_history.iter().any(|&db| db > self.off_threshold) {
                return true;
            }
        }
        false
    }
}

pub fn process_audio_chunk<S: AudioInputStream>(vu_meter: &mut VUMeter<S>) -> Option<(Vec<ChannelMetrics>, Vec<Vec<i32>>)> {
    let audio = vu_meter.read_audio_chunk()?;
    let mut metrics = Vec::new();

    for (ch, channel_data) in audio.iter().enumerate() {
        let db = vu_meter.calculate_db(channel_data);
        let peak_db = vu_meter.calculate_peak_db(channel_data);
        let is_clipping = vu_meter.detect_clipping(channel_data);
        let (max_db, max_peak_db, is_on, has_clipped) =
            vu_meter.update_history(ch, db, peak_db, is_clipping);

        metrics.push(ChannelMetrics {
            db,
            peak_db,
            max_db,
            max_peak_db,
            is_on,
            has_clipped,
        });
    }

    Some((metrics, audio))
}

#[derive(Debug)]
pub struct ChannelMetrics {
    pub db: f64,
    pub peak_db: f64,
    pub max_db: f64,
    pub max_peak_db: f64,
    pub is_on: bool,
    pub has_clipped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_stream::{AudioStream, PipeWireInputStream};

    fn create_test_meter() -> VUMeter<PipeWireInputStream> {
        let stream = PipeWireInputStream::new(
            "test_target".to_string(),
            48000,
            2,
            SampleFormat::S32,
        ).expect("Failed to create PipeWireInputStream");
        VUMeter::new(stream, 0.1, 90.0, 0.0, -60.0, 10.0)
    }

    #[test]
    fn test_sample_format_from_str() {
        assert!(matches!(
            SampleFormat::from_str("s16"),
            Ok(SampleFormat::S16)
        ));
        assert!(matches!(
            SampleFormat::from_str("s16le"),
            Ok(SampleFormat::S16)
        ));
        assert!(matches!(
            SampleFormat::from_str("s32"),
            Ok(SampleFormat::S32)
        ));
        assert!(matches!(
            SampleFormat::from_str("s32le"),
            Ok(SampleFormat::S32)
        ));
        assert!(SampleFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_sample_format_properties() {
        assert_eq!(SampleFormat::S16.bytes_per_sample(), 2);
        assert_eq!(SampleFormat::S32.bytes_per_sample(), 4);
        assert_eq!(SampleFormat::S16.max_value(), 32768.0);
        assert_eq!(SampleFormat::S32.max_value(), 2147483648.0);
        assert_eq!(SampleFormat::S16.as_str(), "s16");
        assert_eq!(SampleFormat::S32.as_str(), "s32");
    }

    #[test]
    fn test_vu_meter_creation() {
        let stream = PipeWireInputStream::new(
            "test_target".to_string(),
            48000,
            2,
            SampleFormat::S32,
        ).expect("Failed to create PipeWireInputStream");
        let meter = VUMeter::new(stream, 0.1, 90.0, 0.0, -60.0, 10.0);

        assert_eq!(meter.stream.sample_rate(), 48000);
        assert_eq!(meter.stream.channels(), 2);
        assert_eq!(meter.update_interval, 0.1);
        assert_eq!(meter.db_range, 90.0);
        assert_eq!(meter.max_db, 0.0);
        assert_eq!(meter.min_db, -90.0);
    }

    #[test]
    fn test_calculate_db() {
        let meter = create_test_meter();

        // Test with silence (should return min_db)
        let silence = vec![0i32; 100];
        let db = meter.calculate_db(&silence);
        assert_eq!(db, meter.min_db);

        // Test with max signal
        let max_signal = vec![i32::MAX / 2; 100];
        let db = meter.calculate_db(&max_signal);
        assert!(db > -10.0); // Should be close to 0dB

        // Test with medium signal
        let medium_signal = vec![1000000i32; 100];
        let db = meter.calculate_db(&medium_signal);
        assert!(db > meter.min_db && db < meter.max_db);
    }

    #[test]
    fn test_calculate_peak_db() {
        let meter = create_test_meter();

        let signal = vec![0, 100000, -200000, 50000];
        let peak_db = meter.calculate_peak_db(&signal);
        
        // Peak should be based on 200000
        assert!(peak_db > meter.min_db);
        assert!(peak_db < meter.max_db);
    }

    #[test]
    fn test_detect_clipping() {
        let meter = create_test_meter();

        // No clipping
        let normal = vec![100000i32; 100];
        assert!(!meter.detect_clipping(&normal));

        // With clipping (99.9% of max value)
        let clipping_threshold = (0.999 * meter.stream.sample_format().max_value()) as i32;
        let clipped = vec![clipping_threshold + 1000; 100];
        assert!(meter.detect_clipping(&clipped));
    }

    #[test]
    fn test_update_history() {
        let mut meter = create_test_meter();

        // Add some values to history
        let (max_db, max_peak_db, is_on, has_clipped) =
            meter.update_history(0, -30.0, -25.0, false);
        
        assert_eq!(max_db, -30.0);
        assert_eq!(max_peak_db, -25.0);
        assert!(is_on); // -30.0 > -60.0 threshold
        assert!(!has_clipped);

        // Add a clipping event
        let (_, _, _, has_clipped) = meter.update_history(0, -20.0, -15.0, true);
        assert!(has_clipped);
    }

    #[test]
    fn test_is_any_channel_on() {
        let mut meter = create_test_meter();

        // Initially off
        assert!(!meter.is_any_channel_on());

        // Add signal above threshold to channel 0
        meter.update_history(0, -30.0, -25.0, false);
        assert!(meter.is_any_channel_on());
    }

    #[test]
    fn test_channel_metrics() {
        let metrics = ChannelMetrics {
            db: -20.0,
            peak_db: -15.0,
            max_db: -18.0,
            max_peak_db: -12.0,
            is_on: true,
            has_clipped: false,
        };

        assert_eq!(metrics.db, -20.0);
        assert!(metrics.is_on);
        assert!(!metrics.has_clipped);
    }
}

