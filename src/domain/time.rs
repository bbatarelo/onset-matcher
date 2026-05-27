/// Position in musical beat space.
/// Beat 0.0 is the first beat of the piece.
/// Beat 1.0 is the second beat, etc. (0-indexed, floating point for sub-beat precision).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Beat(pub f64);

/// Position in audio time (seconds from the start of the audio file).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Seconds(pub f64);

/// Position as a sample frame index (0-indexed from start of audio file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FrameIndex(pub usize);

/// Standard time signature: numerator/denominator (e.g. 4/4, 3/4, 6/8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    /// Beats per bar (e.g. 4 in 4/4).
    pub numerator: u8,
    /// Note value of one beat (e.g. 4 = quarter note, 8 = eighth note).
    pub denominator: u8,
}

impl TimeSignature {
    pub fn new(numerator: u8, denominator: u8) -> Self {
        Self { numerator, denominator }
    }

    /// Beats per bar as f64.
    pub fn beats_per_bar(&self) -> f64 {
        self.numerator as f64
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self { numerator: 4, denominator: 4 }
    }
}

impl std::fmt::Display for TimeSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// The bridge between audio time (seconds) and musical time (beats).
///
/// Assumes constant tempo. `beat_zero_seconds` is where beat 0.0 falls in the audio file.
/// This is typically estimated by looking at where the first strong onset occurs,
/// or supplied by the user.
///
/// Conversions:
///   seconds = beat_zero_seconds + beat * 60.0 / bpm
///   beat    = (seconds - beat_zero_seconds) / (60.0 / bpm)
#[derive(Debug, Clone)]
pub struct TimeMap {
    pub bpm: f64,
    pub time_signature: TimeSignature,
    /// Position in the audio file (seconds) that corresponds to beat 0.0.
    pub beat_zero_seconds: Seconds,
}

impl TimeMap {
    pub fn new(bpm: f64, time_signature: TimeSignature, beat_zero_seconds: Seconds) -> Self {
        Self { bpm, time_signature, beat_zero_seconds }
    }

    /// Seconds per beat at this tempo.
    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.bpm
    }

    /// Convert a beat position to audio seconds.
    pub fn beat_to_seconds(&self, beat: Beat) -> Seconds {
        Seconds(self.beat_zero_seconds.0 + beat.0 * self.seconds_per_beat())
    }

    /// Convert audio seconds to a beat position.
    pub fn seconds_to_beat(&self, seconds: Seconds) -> Beat {
        Beat((seconds.0 - self.beat_zero_seconds.0) / self.seconds_per_beat())
    }

    /// Bar number (0-indexed) for a given beat.
    pub fn beat_to_bar(&self, beat: Beat) -> u32 {
        (beat.0 / self.time_signature.beats_per_bar()).floor() as u32
    }

    /// Beat-within-bar (0-indexed, 0.0 = first beat of bar) for a given beat.
    pub fn beat_to_beat_in_bar(&self, beat: Beat) -> f64 {
        beat.0 % self.time_signature.beats_per_bar()
    }

    /// Total number of complete bars in a given duration in seconds.
    pub fn bars_in_seconds(&self, duration: Seconds) -> u32 {
        let total_beats = duration.0 / self.seconds_per_beat();
        (total_beats / self.time_signature.beats_per_bar()).floor() as u32
    }
}
