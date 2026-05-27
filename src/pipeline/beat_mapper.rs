use crate::domain::audio::{GridOnset, ObservedOnset};
use crate::domain::time::{Beat, Seconds, TimeMap, TimeSignature};

/// Build a `TimeMap` from user-supplied parameters.
///
/// `beat_zero_seconds`: where beat 0 falls in the audio file.
/// For a first approximation this is 0.0 (audio starts exactly on beat 0).
/// It can be refined later by aligning the first detected onset to the nearest
/// expected beat position.
pub fn build_time_map(
    bpm: f64,
    time_signature: TimeSignature,
    beat_zero_seconds: Seconds,
) -> TimeMap {
    TimeMap::new(bpm, time_signature, beat_zero_seconds)
}

/// Map a list of `ObservedOnset`s (in audio seconds) to `GridOnset`s (in beat space)
/// using the provided `TimeMap`.
///
/// For each onset, the nearest grid beat (at the given subdivision resolution)
/// is found, and `quantization_error_beats` is computed as (actual beat - nearest grid beat).
///
/// `grid_subdivision`: the beat grid resolution for quantization (e.g. 0.25 = 16th notes,
/// 0.5 = 8th notes, 1.0 = quarter notes).
pub fn map_onsets_to_grid(
    onsets: &[ObservedOnset],
    time_map: &TimeMap,
    grid_subdivision: f64,
) -> Vec<GridOnset> {
    onsets
        .iter()
        .map(|onset| {
            let beat = time_map.seconds_to_beat(onset.time_seconds);
            let bar = time_map.beat_to_bar(beat);
            let beat_in_bar = time_map.beat_to_beat_in_bar(beat);

            // Nearest grid beat (quantized to the subdivision).
            let nearest_grid_beat = (beat.0 / grid_subdivision).round() * grid_subdivision;
            let quantization_error = beat.0 - nearest_grid_beat;

            GridOnset {
                time_seconds: onset.time_seconds,
                beat,
                bar,
                beat_in_bar,
                strength: onset.strength,
                quantization_error_beats: quantization_error,
            }
        })
        .collect()
}

/// Attempt to refine `beat_zero_seconds` by aligning the first strong onset to
/// the nearest expected beat boundary.
///
/// This corrects for a recording that doesn't start exactly on beat 0.
/// Returns a new `TimeMap` with the adjusted `beat_zero_seconds`.
///
/// Strategy: take the first onset above `strength_threshold`, assume it falls on
/// beat `N` (the nearest integer beat from the naive estimate), then shift
/// `beat_zero_seconds` so that beat `N` lands exactly at that onset's time.
pub fn refine_beat_zero(
    onsets: &[ObservedOnset],
    time_map: &TimeMap,
    strength_threshold: f32,
) -> TimeMap {
    // Find the first onset above the threshold.
    let first_strong = onsets.iter().find(|o| o.strength >= strength_threshold);

    let Some(onset) = first_strong else {
        // No strong onset found; return the original time map unchanged.
        return time_map.clone();
    };

    // What beat does this onset map to with the current (unrefined) time map?
    let raw_beat = time_map.seconds_to_beat(onset.time_seconds);

    // Snap to the nearest integer beat.
    let snapped_beat = Beat(raw_beat.0.round());

    // Compute the new beat_zero_seconds so that snapped_beat falls at onset.time_seconds.
    // beat_to_seconds(snapped_beat) == onset.time_seconds
    // => beat_zero_seconds + snapped_beat * spb == onset.time_seconds
    // => beat_zero_seconds == onset.time_seconds - snapped_beat * spb
    let new_beat_zero = Seconds(onset.time_seconds.0 - snapped_beat.0 * time_map.seconds_per_beat());

    TimeMap::new(time_map.bpm, time_map.time_signature, new_beat_zero)
}
