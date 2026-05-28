use crate::domain::midi::SourceId;
use crate::domain::time::{Beat, TimeMap};

/// Unique identifier for an arrangement layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayerId(pub String);

impl LayerId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One placed instance of a `MidiSource` on the global beat timeline.
///
/// An `ArrangementLayer` says:
///   "Play `source_id` starting at `start_beat` in the global timeline,
///    ending at `end_beat`, beginning from `source_start_beat` within the file."
///
/// Multiple layers may overlap — two MIDI files can be active simultaneously.
#[derive(Debug, Clone)]
pub struct ArrangementLayer {
    pub layer_id: LayerId,
    pub source_id: SourceId,
    /// Where this layer begins on the global beat timeline.
    pub start_beat: Beat,
    /// Where this layer ends on the global beat timeline.
    pub end_beat: Beat,
    /// Which beat within the MIDI source to start playback from.
    /// Usually 0.0, but can be non-zero to use a portion of the MIDI file.
    pub source_start_beat: Beat,
    /// Optional gain adjustment (1.0 = no change).
    pub gain: Option<f32>,
    /// Optional human-readable role label (e.g. "verse", "fill", "transition").
    pub role: Option<String>,
}

impl ArrangementLayer {
    /// Duration of this layer in beats.
    pub fn duration_beats(&self) -> f64 {
        self.end_beat.0 - self.start_beat.0
    }

    /// True if this layer is active at the given global beat.
    pub fn is_active_at(&self, beat: Beat) -> bool {
        beat.0 >= self.start_beat.0 && beat.0 < self.end_beat.0
    }
}

/// A complete arrangement: a `TimeMap` plus a collection of placed MIDI layers.
///
/// Layers may overlap freely. The arrangement describes *what plays when*
/// but not the audio waveform.
#[derive(Debug, Clone)]
pub struct Arrangement {
    pub time_map: TimeMap,
    pub layers: Vec<ArrangementLayer>,
}

impl Arrangement {
    pub fn new(time_map: TimeMap) -> Self {
        Self { time_map, layers: Vec::new() }
    }

    /// Total span of the arrangement in beats (end of last layer).
    pub fn total_beats(&self) -> f64 {
        self.layers
            .iter()
            .map(|l| l.end_beat.0)
            .fold(0.0_f64, f64::max)
    }

    /// Layers active at a given beat position.
    pub fn active_layers_at(&self, beat: Beat) -> Vec<&ArrangementLayer> {
        self.layers
            .iter()
            .filter(|l| l.is_active_at(beat))
            .collect()
    }
}
