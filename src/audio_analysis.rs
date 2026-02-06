//! Audio analysis utilities for RMS computation and signal level estimation.

use crate::SampleFormat;

/// Compute RMS in dB for a chunk of audio samples.
///
/// # Arguments
/// * `audio` - Multi-channel audio samples (outer vec = channels, inner vec = samples)
/// * `format` - Sample format (S16 or S32)
///
/// # Returns
/// RMS level in dB, or -80 dB if no samples
pub fn compute_rms_db(audio: &[Vec<i32>], format: SampleFormat) -> f32 {
    let num_channels = audio.len();
    let num_samples = audio[0].len();
    
    if num_samples == 0 {
        return -80.0;
    }
    
    let max_value = match format {
        SampleFormat::S16 => 32768.0_f32,
        SampleFormat::S32 => 2147483648.0_f32,
    };
    
    let mut sum_squares = 0.0_f64;
    for i in 0..num_samples {
        let mut sample_sum = 0.0_f32;
        for channel in audio {
            sample_sum += channel[i] as f32 / max_value;
        }
        let mono_sample = sample_sum / num_channels as f32;
        sum_squares += (mono_sample * mono_sample) as f64;
    }
    
    let rms = (sum_squares / num_samples as f64).sqrt() as f32;
    
    if rms > 0.0 {
        20.0 * rms.log10()
    } else {
        -80.0
    }
}

/// Apply a moving average smoothing filter in the linear domain.
///
/// Converts dB to linear, applies moving average, then converts back to dB.
///
/// # Arguments
/// * `rms_values` - RMS values in dB
/// * `window_size` - Size of the smoothing window
///
/// # Returns
/// Smoothed RMS values in dB
pub fn smooth_rms(rms_values: &[f32], window_size: usize) -> Vec<f32> {
    let half = window_size / 2;
    let len = rms_values.len();
    let mut smoothed = Vec::with_capacity(len);
    
    let linear: Vec<f64> = rms_values.iter()
        .map(|&db| 10.0_f64.powf(db as f64 / 20.0))
        .collect();
    
    for i in 0..len {
        let start = if i > half { i - half } else { 0 };
        let end = (i + half + 1).min(len);
        let sum: f64 = linear[start..end].iter().sum();
        let avg = sum / (end - start) as f64;
        let db = if avg > 0.0 { 20.0 * avg.log10() } else { -80.0 };
        smoothed.push(db as f32);
    }
    
    smoothed
}

/// Estimate the noise floor level from smoothed RMS data.
///
/// Uses the 5th-10th percentile of RMS values to estimate background noise.
///
/// # Arguments
/// * `smoothed` - Smoothed RMS values in dB
///
/// # Returns
/// Estimated noise floor level in dB
pub fn estimate_noise_floor(smoothed: &[f32]) -> f32 {
    let mut sorted = smoothed.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p5 = (sorted.len() as f64 * 0.05) as usize;
    let p10 = (sorted.len() as f64 * 0.10) as usize;
    if p10 > p5 {
        sorted[p5..=p10].iter().sum::<f32>() / (p10 - p5 + 1) as f32
    } else {
        sorted[p5.min(sorted.len() - 1)]
    }
}

/// Estimate the music level from smoothed RMS data.
///
/// Uses the 60th-80th percentile of RMS values to estimate typical music level.
///
/// # Arguments
/// * `smoothed` - Smoothed RMS values in dB
///
/// # Returns
/// Estimated music level in dB
pub fn estimate_music_level(smoothed: &[f32]) -> f32 {
    let mut sorted = smoothed.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p60 = (sorted.len() as f64 * 0.60) as usize;
    let p80 = (sorted.len() as f64 * 0.80) as usize;
    if p80 > p60 {
        sorted[p60..=p80].iter().sum::<f32>() / (p80 - p60 + 1) as f32
    } else {
        sorted[p60.min(sorted.len() - 1)]
    }
}
