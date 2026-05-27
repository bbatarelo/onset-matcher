use crate::domain::audio::{AudioOnsetCurve, ObservedOnset};

/// Configuration for the peak-picking algorithm.
pub struct PeakPickerConfig {
    /// Minimum onset strength to be considered a peak (0.0–1.0).
    pub threshold: f32,
    /// Minimum number of frames between two successive peaks.
    /// Prevents double-detection of the same onset.
    pub min_gap_frames: usize,
    /// Number of frames on each side that must be lower for a point to be a local max.
    pub local_max_window: usize,
}

impl Default for PeakPickerConfig {
    fn default() -> Self {
        Self {
            threshold: 0.15,
            min_gap_frames: 4,   // ~23 ms at 512 hop / 44100 Hz
            local_max_window: 2,
        }
    }
}

/// Detect discrete onset peaks from an `AudioOnsetCurve`.
///
/// Algorithm: local maximum detection with a minimum threshold and a minimum
/// inter-onset gap to suppress multiple detections of the same transient.
pub fn detect_onsets(curve: &AudioOnsetCurve, config: &PeakPickerConfig) -> Vec<ObservedOnset> {
    let values = &curve.values;
    let n = values.len();
    let w = config.local_max_window;

    let mut onsets: Vec<ObservedOnset> = Vec::new();
    let mut last_onset_frame: Option<usize> = None;

    for i in w..n.saturating_sub(w) {
        let v = values[i];

        // Must exceed the absolute threshold.
        if v < config.threshold {
            continue;
        }

        // Must be a local maximum within the window.
        let is_local_max = (1..=w).all(|d| values[i] >= values[i - d] && values[i] >= values[i + d]);
        if !is_local_max {
            continue;
        }

        // Must be at least min_gap_frames away from the previous onset.
        if let Some(last) = last_onset_frame {
            if i - last < config.min_gap_frames {
                continue;
            }
        }

        let time_seconds = curve.frame_to_seconds(i);
        onsets.push(ObservedOnset {
            time_seconds,
            strength: v,
            width_seconds: estimate_width(values, i, curve.hop_seconds()),
            confidence: None,
        });

        last_onset_frame = Some(i);
    }

    onsets
}

/// Estimate the width of a transient by finding how quickly the onset curve
/// falls back below half its peak value on each side.
fn estimate_width(values: &[f32], peak: usize, hop_seconds: f64) -> Option<f64> {
    let half = values[peak] * 0.5;
    let n = values.len();

    // Left side: walk backwards until we drop below half.
    let left = (0..peak).rev().find(|&i| values[i] < half).unwrap_or(0);

    // Right side: walk forwards until we drop below half.
    let right = (peak + 1..n).find(|&i| values[i] < half).unwrap_or(n - 1);

    Some((right - left) as f64 * hop_seconds)
}

/// Compute a simple adaptive threshold: local mean + k * local standard deviation,
/// optionally used instead of the fixed threshold in `PeakPickerConfig`.
#[allow(dead_code)]
fn adaptive_threshold(values: &[f32], center: usize, window: usize, k: f32) -> f32 {
    let half = window / 2;
    let start = center.saturating_sub(half);
    let end = (center + half).min(values.len());
    let slice = &values[start..end];

    let mean = slice.iter().sum::<f32>() / slice.len() as f32;
    let variance = slice.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / slice.len() as f32;
    let std_dev = variance.sqrt();

    mean + k * std_dev
}

/// Helper: convert onset list to a set of per-frame flags (useful for rendering).
pub fn onsets_to_frame_flags(onsets: &[ObservedOnset], curve: &AudioOnsetCurve) -> Vec<bool> {
    let mut flags = vec![false; curve.values.len()];
    for onset in onsets {
        let frame = curve.seconds_to_frame(onset.time_seconds);
        if frame < flags.len() {
            flags[frame] = true;
        }
    }
    flags
}
