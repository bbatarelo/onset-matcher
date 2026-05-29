/// Arrangement search: find the best beat offsets for a set of MIDI sources by
/// scoring each candidate arrangement against the audio onset curve.
///
/// # Algorithm
///
/// For each candidate arrangement (a combination of per-source beat offsets):
/// 1. Render the MIDI note-on events onto an expected-onset frame buffer at the
///    same sample rate / hop size as the audio onset curve.
/// 2. Compute the overlap score: dot product of the expected buffer and the audio
///    onset curve (equivalent to cross-correlation at lag 0 since we already
///    enumerate lags explicitly via the offset search).
/// 3. Keep the highest-scoring candidate.
///
/// # Search space
///
/// Each source is searched over a configurable range of beat offsets with a
/// configurable step size.  For N sources over R beats at step S, the search
/// visits (R/S)^N candidates — keep N and R small or increase S for speed.
use crate::domain::audio::AudioOnsetCurve;
use crate::domain::midi::MidiSource;
use crate::domain::time::TimeMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Parameters governing the search over beat offsets.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Smallest beat offset to try (inclusive). Default: 0.0.
    pub offset_min: f64,
    /// Largest beat offset to try (inclusive). Default: 0.0 (single-point search).
    pub offset_max: f64,
    /// Step between candidate offsets, in beats. Default: 1.0 (one beat).
    pub offset_step: f64,
    /// Width of the Gaussian bump placed at each note-on event, in seconds.
    /// Larger = more tolerant of timing error; smaller = more precise.
    /// Default: 0.025 s (25 ms).
    pub event_window_seconds: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            offset_min:           0.0,
            offset_max:           0.0,
            offset_step:          1.0,
            event_window_seconds: 0.025,
        }
    }
}

/// The result of a successful arrangement search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Best-scoring beat offset for each input MIDI source, in the same order as
    /// the `sources` slice passed to `find_best_arrangement`.
    pub best_offsets: Vec<f64>,
    /// Overlap score of the best arrangement (higher is better; not normalised).
    pub score: f64,
}

// ---------------------------------------------------------------------------
// Core search
// ---------------------------------------------------------------------------

/// Find the beat offsets for `sources` that maximise overlap with `audio_curve`.
///
/// `per_source_candidates` is a per-source list of candidate beat offsets to try.
/// Sources with a single candidate (pinned by the user) are not searched; sources
/// with multiple candidates are searched over those values.  All combinations are
/// tried (exhaustive Cartesian product).
///
/// `event_window_seconds` is the Gaussian bump width used in scoring.
pub fn find_best_arrangement(
    sources: &[MidiSource],
    audio_curve: &AudioOnsetCurve,
    time_map: &TimeMap,
    per_source_candidates: &[Vec<f64>],
    event_window_seconds: f64,
) -> SearchResult {
    let n = sources.len();
    assert_eq!(per_source_candidates.len(), n, "per_source_candidates must have one entry per source");

    // Default best offsets: first candidate for each source.
    let mut best_offsets: Vec<f64> = per_source_candidates
        .iter()
        .map(|cands| cands.first().copied().unwrap_or(0.0))
        .collect();
    let mut best_score = f64::NEG_INFINITY;

    if n == 0 {
        return SearchResult { best_offsets, score: 0.0 };
    }

    // Compute total number of combinations and bail early if any source has no candidates.
    let counts: Vec<usize> = per_source_candidates.iter().map(|c| c.len()).collect();
    if counts.iter().any(|&c| c == 0) {
        return SearchResult { best_offsets, score: 0.0 };
    }
    let total_candidates: usize = counts.iter().product();

    // Enumerate all combinations via a mixed-radix counter (index per source).
    let mut indices = vec![0usize; n];

    for _ in 0..total_candidates {
        let offsets: Vec<f64> = indices
            .iter()
            .enumerate()
            .map(|(i, &idx)| per_source_candidates[i][idx])
            .collect();
        let score = score_arrangement(sources, &offsets, audio_curve, time_map, event_window_seconds);

        if score > best_score {
            best_score = score;
            best_offsets = offsets;
        }

        // Increment mixed-radix counter (last source increments first).
        let mut overflow = true;
        for (idx, &count) in indices.iter_mut().zip(counts.iter()).rev() {
            *idx += 1;
            if *idx >= count {
                *idx = 0;
                // overflow = true; keep propagating carry
            } else {
                overflow = false;
                break;
            }
        }
        // If overflow is still true here, all combinations have been visited —
        // the outer loop will end naturally on the next iteration.
        let _ = overflow;
    }

    SearchResult { best_offsets, score: best_score }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Score a specific arrangement: dot product of expected-onset curve and audio
/// onset curve.
///
/// Expected onset curve is generated by placing a Gaussian bump of width
/// `event_window_seconds` at the audio-time position of every MIDI note-on event
/// in the arrangement.
pub fn score_arrangement(
    sources: &[MidiSource],
    offsets: &[f64],
    audio_curve: &AudioOnsetCurve,
    time_map: &TimeMap,
    event_window_seconds: f64,
) -> f64 {
    let n_frames = audio_curve.values.len();
    if n_frames == 0 {
        return 0.0;
    }

    let mut expected = vec![0.0f32; n_frames];
    let hop = audio_curve.hop_seconds();
    // Sigma in frames (convert window seconds → frames).
    let sigma_frames = (event_window_seconds / hop).max(0.5);

    for (source, &offset_beats) in sources.iter().zip(offsets.iter()) {
        for event in &source.events {
            let global_beat = event.beat.0 + offset_beats;
            let event_seconds = time_map.beat_zero_seconds.0
                + global_beat * 60.0 / time_map.bpm;
            let center_frame = (event_seconds / hop).round() as i64;

            // Add Gaussian bump centred on this frame.
            // 3σ covers 99.7% of the bump; beyond that the contribution is negligible.
            let radius = (3.0 * sigma_frames).ceil() as i64;

            // Skip events whose Gaussian footprint is entirely outside the frame buffer.
            // This handles negative center_frame (MIDI note before beat_zero) and
            // center_frame >= n_frames (MIDI note after audio ends).
            // Without this guard, casting a negative i64 to usize wraps around and
            // causes an out-of-bounds panic.
            if center_frame + radius < 0 || center_frame - radius >= n_frames as i64 {
                continue;
            }

            let lo = (center_frame - radius).max(0) as usize;
            let hi = (center_frame + radius).min(n_frames as i64 - 1) as usize;

            for f in lo..=hi {
                let d = (f as f64 - center_frame as f64) / sigma_frames;
                let w = (-0.5 * d * d).exp() as f32;
                expected[f] += w;
            }
        }
    }

    // Dot product.
    expected.iter()
        .zip(audio_curve.values.iter())
        .map(|(&e, &a)| (e * a) as f64)
        .sum()
}

// ---------------------------------------------------------------------------
// Convenience: score with explicit per-source offsets (for score-arrangement)
// ---------------------------------------------------------------------------

/// Convenience wrapper: score a specific set of offsets against the audio curve.
/// Returns the dot-product overlap score (higher is better).
pub fn score_offsets(
    sources: &[MidiSource],
    offsets: &[f64],
    audio_curve: &AudioOnsetCurve,
    time_map: &TimeMap,
) -> f64 {
    score_arrangement(sources, offsets, audio_curve, time_map, 0.025)
}

// ---------------------------------------------------------------------------
// Expected-onset curve rendering (for diagnostic display)
// ---------------------------------------------------------------------------

/// Render the expected-onset curve for a given arrangement as a `Vec<f32>` with
/// the same frame count and hop size as `audio_curve`.
///
/// Useful for visual comparison: render both curves as sparklines side by side.
pub fn render_expected_curve(
    sources: &[MidiSource],
    offsets: &[f64],
    audio_curve: &AudioOnsetCurve,
    time_map: &TimeMap,
    event_window_seconds: f64,
) -> Vec<f32> {
    let n_frames = audio_curve.values.len();
    let mut expected = vec![0.0f32; n_frames];
    let hop = audio_curve.hop_seconds();
    let sigma_frames = (event_window_seconds / hop).max(0.5);

    for (source, &offset_beats) in sources.iter().zip(offsets.iter()) {
        for event in &source.events {
            let global_beat = event.beat.0 + offset_beats;
            let event_seconds = time_map.beat_zero_seconds.0
                + global_beat * 60.0 / time_map.bpm;
            let center_frame = (event_seconds / hop).round() as i64;

            let radius = (3.0 * sigma_frames).ceil() as i64;

            // Skip events whose Gaussian footprint is entirely outside the frame buffer.
            if center_frame + radius < 0 || center_frame - radius >= n_frames as i64 {
                continue;
            }

            let lo = (center_frame - radius).max(0) as usize;
            let hi = (center_frame + radius).min(n_frames as i64 - 1) as usize;

            for f in lo..=hi {
                let d = (f as f64 - center_frame as f64) / sigma_frames;
                let w = (-0.5 * d * d).exp() as f32;
                expected[f] += w;
            }
        }
    }
    expected
}
