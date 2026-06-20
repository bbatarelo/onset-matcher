use crate::domain::arrangement::LayerId;
use crate::domain::midi::SourceId;
use crate::domain::time::Beat;

/// One MIDI note-on event rendered onto the global beat timeline.
///
/// Produced by flattening all `ArrangementLayer`s + `MidiSource`s into a
/// single sorted list, suitable for export and template generation.
#[derive(Debug, Clone)]
pub struct ExpectedEvent {
    /// Position on the global beat timeline.
    pub beat: Beat,
    /// Which MIDI source this event came from.
    pub source_id: SourceId,
    /// Which arrangement layer placed this event.
    pub layer_id: LayerId,
    /// The beat position *inside* the source MIDI file.
    pub source_beat: Beat,
    /// MIDI note number (0–127).
    pub note: u8,
    /// MIDI velocity (1–127).
    pub velocity: u8,
}

/// A group of `ExpectedEvent`s that are simultaneous (or near-simultaneous,
/// within one subdivision cell) on the global beat timeline.
///
/// Multiple MIDI notes fired at the same beat (e.g. kick + hihat) produce
/// one audio transient, so they are treated as one cluster for matching.
#[derive(Debug, Clone)]
pub struct ExpectedOnsetCluster {
    /// Nominal beat position of this cluster.
    pub beat: Beat,
    /// Indices into the global `ExpectedEvent` list.
    pub event_indices: Vec<usize>,
    /// Normalized expected onset strength (0.0–1.0).
    /// Derived from the `MidiOnsetTemplate` at this cluster's frame.
    pub expected_strength: f64,
}

/// A frame-based onset-strength curve generated from an `Arrangement`.
///
/// Parallel in structure to `AudioOnsetCurve`: the same frame rate, same
/// length.  Each frame holds the sum of Gaussian bumps placed at every
/// MIDI note-on event's time position.  Used as a template to score
/// against the audio onset curve.
#[derive(Debug, Clone)]
pub struct MidiOnsetTemplate {
    /// Per-frame onset strength values.
    pub values: Vec<f32>,
    /// Hop size in samples (number of audio samples between consecutive frames).
    pub hop_size: usize,
    /// Audio sample rate in Hz.
    pub sample_rate: u32,
}

impl MidiOnsetTemplate {
    /// Duration of this template in seconds.
    pub fn duration_seconds(&self) -> f64 {
        self.values.len() as f64 * self.hop_size as f64 / self.sample_rate as f64
    }
}
