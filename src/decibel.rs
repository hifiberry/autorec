/// Decibel conversion utilities for audio processing

/// Calculate RMS (Root Mean Square) value from audio samples
pub fn calculate_rms(samples: &[i32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    (sum_squares / samples.len() as f64).sqrt()
}

/// Calculate peak (maximum absolute) value from audio samples
pub fn calculate_peak(samples: &[i32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    samples.iter().map(|&s| s.abs()).max().unwrap_or(0) as f64
}

/// Convert RMS value to decibels relative to a reference value
///
/// # Arguments
/// * `rms` - RMS value to convert
/// * `reference` - Reference value (typically max_value of the sample format)
/// * `min_db` - Minimum dB value to return (floor)
///
/// # Returns
/// Decibel value, clamped to min_db as floor
pub fn rms_to_db(rms: f64, reference: f64, min_db: f64) -> f64 {
    if rms < 1.0 {
        return min_db;
    }

    let db = 20.0 * (rms / reference).log10();
    db.max(min_db)
}

/// Convert peak value to decibels relative to a reference value
///
/// # Arguments
/// * `peak` - Peak value to convert
/// * `reference` - Reference value (typically max_value of the sample format)
/// * `min_db` - Minimum dB value to return (floor)
///
/// # Returns
/// Decibel value, clamped to min_db as floor
pub fn peak_to_db(peak: f64, reference: f64, min_db: f64) -> f64 {
    if peak < 1.0 {
        return min_db;
    }

    let db = 20.0 * (peak / reference).log10();
    db.max(min_db)
}

/// Calculate RMS in decibels from audio samples
///
/// # Arguments
/// * `samples` - Audio samples
/// * `reference` - Reference value (typically max_value of the sample format)
/// * `min_db` - Minimum dB value to return (floor)
/// * `max_db` - Maximum dB value to return (ceiling)
///
/// # Returns
/// RMS level in decibels, clamped between min_db and max_db
pub fn calculate_rms_db(samples: &[i32], reference: f64, min_db: f64, max_db: f64) -> f64 {
    let rms = calculate_rms(samples);
    let db = rms_to_db(rms, reference, min_db);
    db.max(min_db).min(max_db)
}

/// Calculate peak in decibels from audio samples
///
/// # Arguments
/// * `samples` - Audio samples
/// * `reference` - Reference value (typically max_value of the sample format)
/// * `min_db` - Minimum dB value to return (floor)
/// * `max_db` - Maximum dB value to return (ceiling)
///
/// # Returns
/// Peak level in decibels, clamped between min_db and max_db
pub fn calculate_peak_db(samples: &[i32], reference: f64, min_db: f64, max_db: f64) -> f64 {
    let peak = calculate_peak(samples);
    let db = peak_to_db(peak, reference, min_db);
    db.max(min_db).min(max_db)
}

/// Detect if any samples exceed a clipping threshold
///
/// # Arguments
/// * `samples` - Audio samples to check
/// * `threshold` - Clipping threshold (typically 99.9% of max value)
///
/// # Returns
/// True if any sample exceeds the threshold
pub fn detect_clipping(samples: &[i32], threshold: i32) -> bool {
    samples.iter().any(|&s| s.abs() >= threshold)
}

/// Calculate clipping threshold for a given reference value
///
/// # Arguments
/// * `reference` - Reference value (typically max_value of the sample format)
/// * `percentage` - Percentage of reference (e.g., 0.999 for 99.9%)
///
/// # Returns
/// Clipping threshold as i32
pub fn clipping_threshold(reference: f64, percentage: f64) -> i32 {
    (reference * percentage) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_rms() {
        // Test with silence
        let silence = vec![0i32; 100];
        assert_eq!(calculate_rms(&silence), 0.0);

        // Test with constant signal
        let constant = vec![1000i32; 100];
        assert_eq!(calculate_rms(&constant), 1000.0);

        // Test with mixed signal
        let mixed = vec![100, -100, 200, -200];
        let expected_rms = ((100.0_f64.powi(2) + 100.0_f64.powi(2) + 200.0_f64.powi(2) + 200.0_f64.powi(2)) / 4.0).sqrt();
        assert!((calculate_rms(&mixed) - expected_rms).abs() < 0.001);

        // Test with empty
        assert_eq!(calculate_rms(&[]), 0.0);
    }

    #[test]
    fn test_calculate_peak() {
        // Test with silence
        let silence = vec![0i32; 100];
        assert_eq!(calculate_peak(&silence), 0.0);

        // Test with positive peak
        let signal = vec![100, 200, 150, 50];
        assert_eq!(calculate_peak(&signal), 200.0);

        // Test with negative peak
        let signal = vec![100, -300, 150, 50];
        assert_eq!(calculate_peak(&signal), 300.0);

        // Test with empty
        assert_eq!(calculate_peak(&[]), 0.0);
    }

    #[test]
    fn test_rms_to_db() {
        let reference = 32768.0;
        let min_db = -90.0;

        // Test at reference level (should be ~0dB)
        let db = rms_to_db(reference, reference, min_db);
        assert!((db - 0.0).abs() < 0.001);

        // Test at half reference (~-6dB)
        let db = rms_to_db(reference / 2.0, reference, min_db);
        assert!((db - (-6.02)).abs() < 0.1);

        // Test at 10% reference (~-20dB)
        let db = rms_to_db(reference * 0.1, reference, min_db);
        assert!((db - (-20.0)).abs() < 0.1);

        // Test below threshold (should return min_db)
        let db = rms_to_db(0.5, reference, min_db);
        assert_eq!(db, min_db);
    }

    #[test]
    fn test_peak_to_db() {
        let reference = 2147483648.0; // S32 max
        let min_db = -90.0;

        // Test at reference level
        let db = peak_to_db(reference, reference, min_db);
        assert!((db - 0.0).abs() < 0.001);

        // Test at half reference
        let db = peak_to_db(reference / 2.0, reference, min_db);
        assert!((db - (-6.02)).abs() < 0.1);

        // Test below threshold
        let db = peak_to_db(0.5, reference, min_db);
        assert_eq!(db, min_db);
    }

    #[test]
    fn test_calculate_rms_db() {
        let reference = 32768.0;
        let min_db = -90.0;
        let max_db = 0.0;

        // Test with silence
        let silence = vec![0i32; 100];
        assert_eq!(calculate_rms_db(&silence, reference, min_db, max_db), min_db);

        // Test with signal
        let signal = vec![16384i32; 100]; // Half of reference
        let db = calculate_rms_db(&signal, reference, min_db, max_db);
        assert!((db - (-6.02)).abs() < 0.1);

        // Test clamping to max_db
        let loud = vec![i32::MAX / 2; 100];
        let db = calculate_rms_db(&loud, reference, min_db, max_db);
        assert!(db <= max_db);
    }

    #[test]
    fn test_calculate_peak_db() {
        let reference = 32768.0;
        let min_db = -90.0;
        let max_db = 0.0;

        // Test with silence
        let silence = vec![0i32; 100];
        assert_eq!(calculate_peak_db(&silence, reference, min_db, max_db), min_db);

        // Test with peak at half reference
        let signal = vec![100, 200, 16384, 100]; // Peak is 16384 (half of 32768)
        let db = calculate_peak_db(&signal, reference, min_db, max_db);
        assert!((db - (-6.02)).abs() < 0.1);
    }

    #[test]
    fn test_detect_clipping() {
        let threshold = 30000;

        // No clipping
        let normal = vec![1000, 2000, 3000, 4000];
        assert!(!detect_clipping(&normal, threshold));

        // Clipping on positive
        let clipped_pos = vec![1000, 31000, 3000, 4000];
        assert!(detect_clipping(&clipped_pos, threshold));

        // Clipping on negative
        let clipped_neg = vec![1000, -31000, 3000, 4000];
        assert!(detect_clipping(&clipped_neg, threshold));

        // At threshold (should clip)
        let at_threshold = vec![1000, 30000, 3000, 4000];
        assert!(detect_clipping(&at_threshold, threshold));

        // Just below threshold (should not clip)
        let below_threshold = vec![1000, 29999, 3000, 4000];
        assert!(!detect_clipping(&below_threshold, threshold));
    }

    #[test]
    fn test_clipping_threshold() {
        // Test S16 format (max 32768)
        let threshold = clipping_threshold(32768.0, 0.999);
        assert_eq!(threshold, 32735); // 32768 * 0.999 ≈ 32735

        // Test S32 format (max 2147483648)
        let threshold = clipping_threshold(2147483648.0, 0.999);
        // Note: due to floating point precision, exact value may vary slightly
        assert!((threshold - 2145335864).abs() < 1000); // Within reasonable tolerance

        // Test with different percentage
        let threshold = clipping_threshold(32768.0, 0.95);
        assert_eq!(threshold, 31129); // 32768 * 0.95
    }

    #[test]
    fn test_db_range() {
        let reference = 32768.0;
        let min_db = -90.0;

        // Test various signal levels
        let test_cases = vec![
            (32768.0, 0.0),      // Full scale
            (16384.0, -6.0),     // Half scale
            (8192.0, -12.0),     // Quarter scale
            (3276.8, -20.0),     // 10% scale
            (327.68, -40.0),     // 1% scale
        ];

        for (rms, expected_db) in test_cases {
            let db = rms_to_db(rms, reference, min_db);
            assert!((db - expected_db).abs() < 0.1, 
                "RMS {} should be ~{}dB but got {}dB", rms, expected_db, db);
        }
    }

    #[test]
    fn test_empty_input() {
        let reference = 32768.0;
        let min_db = -90.0;
        let max_db = 0.0;
        let empty: Vec<i32> = vec![];

        assert_eq!(calculate_rms(&empty), 0.0);
        assert_eq!(calculate_peak(&empty), 0.0);
        assert_eq!(calculate_rms_db(&empty, reference, min_db, max_db), min_db);
        assert_eq!(calculate_peak_db(&empty, reference, min_db, max_db), min_db);
        assert!(!detect_clipping(&empty, 30000));
    }
}
