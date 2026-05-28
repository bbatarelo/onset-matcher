use std::path::PathBuf;
use crate::domain::time::Beat;

/// Unique identifier for a MIDI source file.
/// Typically derived from the filename stem or assigned by the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

impl SourceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A single MIDI note-on event expressed in musical-time coordinates.
///
/// Velocity 0 note-ons (which MIDI spec treats as note-off) are excluded
/// during parsing.
#[derive(Debug, Clone)]
pub struct MidiEvent {
    /// Beat position within the MIDI source (0.0 = start of the file).
    pub beat: Beat,
    /// MIDI note number (0–127). For drums this is typically the instrument number.
    pub note: u8,
    /// Velocity (1–127). 0 = note-off; those are excluded from the event list.
    pub velocity: u8,
    /// MIDI channel (0-indexed, 0–15).
    pub channel: u8,
    /// Duration of the note in beats (from note-on to note-off), if determinable.
    pub duration_beats: Option<Beat>,
}

/// A parsed MIDI file expressed as beat-based symbolic material.
///
/// A `MidiSource` is raw material — it has no position on the global timeline.
/// Placement on the global timeline is done by wrapping it in an `ArrangementLayer`.
#[derive(Debug, Clone)]
pub struct MidiSource {
    /// Unique identifier (typically the filename stem).
    pub id: SourceId,
    /// Path to the original MIDI file.
    pub path: PathBuf,
    /// Total length of the MIDI file in beats.
    pub length_beats: Beat,
    /// BPM found in the MIDI file's tempo track (first tempo event).
    /// `None` if no tempo event was found.
    pub embedded_bpm: Option<f64>,
    /// Ticks per quarter note (PPQ) as declared in the MIDI file header.
    pub ppq: u16,
    /// All note-on events sorted by beat position.
    pub events: Vec<MidiEvent>,
}

impl MidiSource {
    /// Number of distinct note values that appear in this source.
    pub fn unique_notes(&self) -> Vec<u8> {
        let mut notes: Vec<u8> = self.events.iter().map(|e| e.note).collect();
        notes.sort_unstable();
        notes.dedup();
        notes
    }

    /// Events at or near a given beat position (within `tolerance` beats).
    pub fn events_near_beat(&self, beat: Beat, tolerance: f64) -> Vec<&MidiEvent> {
        self.events
            .iter()
            .filter(|e| (e.beat.0 - beat.0).abs() <= tolerance)
            .collect()
    }
}
