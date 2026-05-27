use std::path::PathBuf;
use crate::domain::time::{Beat, Seconds};

/// Frame-based onset strength curve extracted from a reference audio file.
///
/// Each value represents the onset (transient) energy in one analysis frame.
/// This is the continuous signal the matcher slides MIDI templates over.
///
/// Frame index i corresponds to audio time: i * hop_size / sample_rate seconds.
#[derive(Debug, Clone)]
pub struct AudioOnsetCurve {
    pub sample_rate: u32,
    /// Size of the hop between analysis frames in samples.
    pub hop_size: usize,
    /// Onset strength per frame. Index 0 = start of audio.
    pub values: Vec<f32>,
}

impl AudioOnsetCurve {
    /// Time in seconds for a given frame index.
    pub fn frame_to_seconds(&self, frame: usize) -> Seconds {
        Seconds(frame as f64 * self.hop_size as f64 / self.sample_rate as f64)
    }

    /// Nearest frame index for a given time in seconds.
    pub fn seconds_to_frame(&self, seconds: Seconds) -> usize {
        let frame = seconds.0 * self.sample_rate as f64 / self.hop_size as f64;
        (frame.round() as usize).min(self.values.len().saturating_sub(1))
    }

    /// Duration of the onset curve in seconds.
    pub fn duration_seconds(&self) -> Seconds {
        Seconds(self.values.len() as f64 * self.hop_size as f64 / self.sample_rate as f64)
    }

    /// Hop duration in seconds.
    pub fn hop_seconds(&self) -> f64 {
        self.hop_size as f64 / self.sample_rate as f64
    }
}

/// A discrete onset peak extracted from an `AudioOnsetCurve`.
///
/// These are the individual transient candidates detected in the reference audio,
/// expressed in audio-time coordinates (seconds).
/// After alignment, they are mapped to musical-time coordinates as `GridOnset`.
#[derive(Debug, Clone)]
pub struct ObservedOnset {
    /// Time of this onset in audio seconds.
    pub time_seconds: Seconds,
    /// Onset strength at the peak (0.0–1.0 normalised, or unnormalised depending on detector).
    pub strength: f32,
    /// Approximate duration of the transient in seconds (optional).
    pub width_seconds: Option<f64>,
    /// Detector confidence in this onset (optional, 0.0–1.0).
    pub confidence: Option<f64>,
}

/// An `ObservedOnset` that has been mapped to musical (beat) coordinates
/// using a `TimeMap`. Computed after the best arrangement is known.
#[derive(Debug, Clone)]
pub struct GridOnset {
    /// Original audio time.
    pub time_seconds: Seconds,
    /// Beat position on the global musical timeline.
    pub beat: Beat,
    /// Bar number (0-indexed).
    pub bar: u32,
    /// Beat within the bar (0.0 = first beat of bar, in beat units).
    pub beat_in_bar: f64,
    /// Onset strength (copied from the source `ObservedOnset`).
    pub strength: f32,
    /// Difference between this onset's beat and the nearest grid beat position.
    /// Positive means the onset was late; negative means early.
    pub quantization_error_beats: f64,
}

/// Container for all audio-derived analysis data for one reference audio file.
#[derive(Debug)]
pub struct AudioAnalysis {
    /// Path to the source audio file.
    pub source_audio: PathBuf,
    /// Decoded sample rate.
    pub sample_rate: u32,
    /// Frame-based onset strength curve (continuous signal).
    pub onset_curve: AudioOnsetCurve,
    /// Discrete onset peaks detected from the curve.
    pub observed_onsets: Vec<ObservedOnset>,
}
