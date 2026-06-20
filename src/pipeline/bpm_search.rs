/// Auto-BPM estimation for the `find-arrangement` pipeline.
///
/// The core problem: the user may not know the exact BPM of the reference audio
/// (e.g. recording clock rate may differ from the nominal project BPM). This
/// module provides a [`BpmEstimator`] trait and a concrete [`AutoGridEstimator`]
/// that exhaustively tries every BPM in a configurable range and picks the one
/// that yields the highest cross-correlation score when combined with
/// `refine_beat_zero` and `find_best_arrangement`.
///
/// # Extension points
///
/// Additional estimators (e.g. median IOI, linear regression) can be added by
/// implementing [`BpmEstimator`] and wiring them to new `--bpm=<mode>` values in
/// `FindArrangementArgs`.

use crate::domain::audio::{AudioOnsetCurve, ObservedOnset};
use crate::domain::midi::MidiSource;
use crate::domain::time::{Seconds, TimeSignature};
use crate::pipeline::beat_mapper::{build_time_map, refine_beat_zero};
use crate::pipeline::search::find_best_arrangement;

// ---------------------------------------------------------------------------
// Offset search configuration
// ---------------------------------------------------------------------------

/// Configures how the beat-offset candidates are built for free (un-pinned) sources.
///
/// When `max_beats` is `None` the upper bound is derived automatically from the
/// audio duration: `floor(audio_duration_seconds * bpm / 60.0)`. This means the
/// search covers every whole-beat start position that still falls within the audio.
///
/// Pinned sources are unaffected — they always receive a single-element candidate
/// list `[pinned_beat]` built in `run_find_arrangement`.
#[derive(Debug, Clone)]
pub struct OffsetSearchConfig {
    /// Minimum beat offset for free sources (inclusive). Default: 0.
    pub min_beats: f64,
    /// Maximum beat offset for free sources (inclusive).
    /// `None` = auto-derive from `floor(audio_duration_seconds * bpm / 60.0)`.
    pub max_beats: Option<f64>,
    /// Step between consecutive beat-offset candidates. Default: 1.0.
    pub step_beats: f64,
    /// Audio file duration in seconds (used to compute auto max when `max_beats` is `None`).
    pub audio_duration_seconds: f64,
}

impl OffsetSearchConfig {
    /// Build the sorted list of candidate beat offsets for a **free** source at a
    /// given BPM.  When `self.max_beats` is `None` the maximum is derived from the
    /// audio duration.
    pub fn free_candidates(&self, bpm: f64) -> Vec<f64> {
        let max = self.max_beats.unwrap_or_else(|| {
            (self.audio_duration_seconds * bpm / 60.0).floor()
        });
        let mut v = Vec::new();
        let mut o = self.min_beats;
        while o <= max + 1e-9 {
            v.push(o);
            o += self.step_beats;
        }
        v
    }
}

// ---------------------------------------------------------------------------
// BPM mode — parsed from the `--bpm` CLI string
// ---------------------------------------------------------------------------

/// How the BPM should be determined for a `find-arrangement` run.
#[derive(Debug, Clone)]
pub enum BpmMode {
    /// Use a fixed BPM (e.g. `--bpm=110`).
    Fixed(f64),
    /// Search a grid of BPM values and pick the best-scoring one
    /// (e.g. `--bpm=auto-grid --bpm-min=80 --bpm-max=140 --bpm-step=1`).
    AutoGrid { min: f64, max: f64, step: f64 },
}

impl BpmMode {
    /// Parse a BPM mode from the raw CLI `--bpm` string.
    ///
    /// Accepted formats:
    /// * A bare positive number — [`BpmMode::Fixed`].
    /// * `"auto-grid"` — [`BpmMode::AutoGrid`] (caller must supply `min`/`max`/`step`).
    pub fn parse(s: &str, grid_min: f64, grid_max: f64, grid_step: f64) -> anyhow::Result<Self> {
        if s.eq_ignore_ascii_case("auto-grid") {
            if grid_step <= 0.0 {
                anyhow::bail!("--bpm-step must be > 0 (got {})", grid_step);
            }
            if grid_max < grid_min {
                anyhow::bail!("--bpm-max ({}) must be >= --bpm-min ({})", grid_max, grid_min);
            }
            if grid_min <= 0.0 {
                anyhow::bail!("--bpm-min must be > 0 (got {})", grid_min);
            }
            Ok(BpmMode::AutoGrid { min: grid_min, max: grid_max, step: grid_step })
        } else {
            let bpm: f64 = s.parse().map_err(|_| {
                anyhow::anyhow!(
                    "Invalid --bpm value {:?}. Expected a positive number or \"auto-grid\".",
                    s
                )
            })?;
            if bpm <= 0.0 {
                anyhow::bail!("--bpm must be > 0 (got {})", bpm);
            }
            Ok(BpmMode::Fixed(bpm))
        }
    }
}

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

/// The output of a [`BpmEstimator`] run.
#[derive(Debug, Clone)]
pub struct BpmSearchResult {
    /// The BPM value that yielded the highest alignment score.
    pub bpm: f64,
    /// The refined `beat_zero_seconds` for that BPM (after `refine_beat_zero`).
    pub beat_zero_seconds: Seconds,
    /// Best beat offsets for each source at the winning BPM.
    pub best_offsets: Vec<f64>,
    /// The best overlap score (higher is better; not normalised).
    pub score: f64,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Pluggable BPM estimator.  Implementations may use different strategies to
/// choose a BPM and arrangement simultaneously.
pub trait BpmEstimator {
    /// Run the estimator and return the best-scoring BPM + arrangement.
    ///
    /// * `onsets` — detected audio onset peaks (used by `refine_beat_zero`).
    /// * `sources` — loaded MIDI sources (passed to `find_best_arrangement`).
    /// * `audio_curve` — raw onset-strength curve (passed to `find_best_arrangement`).
    /// * `time_signature` — time signature to use when building a [`TimeMap`].
    /// * `pinned_offsets` — per-source pinned offset (`Some(beat)`) or free (`None`).
    /// * `offset_config` — controls how free-source candidate lists are built per BPM.
    /// * `event_window_seconds` — Gaussian bump width for the expected-onset template.
    /// * `refine_threshold` — strength threshold passed to `refine_beat_zero`.
    /// * `trim_beat_zero` — when `Some(t)`, pin `beat_zero_seconds = t` for every BPM
    ///   candidate instead of calling `refine_beat_zero`.  This implements `--trim-audio`:
    ///   the first strong onset is placed exactly at beat 0, regardless of BPM.
    fn estimate(
        &self,
        onsets: &[ObservedOnset],
        sources: &[MidiSource],
        audio_curve: &AudioOnsetCurve,
        time_signature: TimeSignature,
        pinned_offsets: &[Option<f64>],
        offset_config: &OffsetSearchConfig,
        event_window_seconds: f64,
        refine_threshold: f32,
        trim_beat_zero: Option<Seconds>,
    ) -> BpmSearchResult;
}

// ---------------------------------------------------------------------------
// AutoGridEstimator
// ---------------------------------------------------------------------------

/// Exhaustive BPM grid search.
///
/// For every BPM in `[min, min+step, min+2·step, …, max]`:
/// 1. Build a [`TimeMap`] with `beat_zero_seconds = 0` (or pinned via `trim_beat_zero`).
/// 2. If no trim: refine `beat_zero` via `refine_beat_zero`.
/// 3. Run `find_best_arrangement` to find the best offsets.
/// 4. Keep the combination with the highest score.
pub struct AutoGridEstimator {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl BpmEstimator for AutoGridEstimator {
    fn estimate(
        &self,
        onsets: &[ObservedOnset],
        sources: &[MidiSource],
        audio_curve: &AudioOnsetCurve,
        time_signature: TimeSignature,
        pinned_offsets: &[Option<f64>],
        offset_config: &OffsetSearchConfig,
        event_window_seconds: f64,
        refine_threshold: f32,
        trim_beat_zero: Option<Seconds>,
    ) -> BpmSearchResult {
        let mut best: Option<BpmSearchResult> = None;

        let mut bpm = self.min;
        while bpm <= self.max + 1e-9 {
            let time_map = if let Some(t) = trim_beat_zero {
                // --trim-audio: place beat 0 exactly at the first strong onset,
                // independently of BPM.  No refine_beat_zero needed.
                build_time_map(bpm, time_signature, t)
            } else if !onsets.is_empty() {
                let base = build_time_map(bpm, time_signature, Seconds(0.0));
                refine_beat_zero(onsets, &base, refine_threshold)
            } else {
                build_time_map(bpm, time_signature, Seconds(0.0))
            };

            // Build per-source candidates for this BPM (free sources get auto-ranged).
            let free_candidates = offset_config.free_candidates(bpm);
            let per_source_candidates: Vec<Vec<f64>> = pinned_offsets
                .iter()
                .map(|pin| match pin {
                    Some(n) => vec![*n],
                    None    => free_candidates.clone(),
                })
                .collect();

            let result = find_best_arrangement(
                sources,
                audio_curve,
                &time_map,
                &per_source_candidates,
                event_window_seconds,
            );

            let is_better = best.as_ref().map_or(true, |b| result.score > b.score);
            if is_better {
                best = Some(BpmSearchResult {
                    bpm,
                    beat_zero_seconds: time_map.beat_zero_seconds,
                    best_offsets: result.best_offsets,
                    score: result.score,
                });
            }

            bpm += self.step;
        }

        // Fallback: if the range was empty (min > max), return a zero result.
        best.unwrap_or(BpmSearchResult {
            bpm: self.min,
            beat_zero_seconds: Seconds(0.0),
            best_offsets: vec![0.0; sources.len()],
            score: 0.0,
        })
    }
}

// ---------------------------------------------------------------------------
// Convenience factory
// ---------------------------------------------------------------------------

/// Construct the appropriate [`BpmEstimator`] from a [`BpmMode`] and run it,
/// returning the final `(bpm, beat_zero_seconds, best_offsets, score)` tuple
/// wrapped in a [`BpmSearchResult`].
///
/// For [`BpmMode::Fixed`] we still call `refine_beat_zero` and
/// `find_best_arrangement` once — this keeps the calling code uniform.
///
/// `trim_beat_zero`: when `Some(t)`, `beat_zero_seconds` is set to `t` for every
/// BPM candidate, bypassing `refine_beat_zero`.  Pass `Some(first_onset.time_seconds)`
/// when `--trim-audio` is active to place beat 0 exactly at the first strong onset.
pub fn resolve_bpm(
    mode: &BpmMode,
    onsets: &[ObservedOnset],
    sources: &[MidiSource],
    audio_curve: &AudioOnsetCurve,
    time_signature: TimeSignature,
    pinned_offsets: &[Option<f64>],
    offset_config: &OffsetSearchConfig,
    event_window_seconds: f64,
    refine_threshold: f32,
    trim_beat_zero: Option<Seconds>,
) -> BpmSearchResult {
    match mode {
        BpmMode::Fixed(bpm) => {
            // Single BPM: build + (optionally trim or refine) + search once.
            let time_map = if let Some(t) = trim_beat_zero {
                build_time_map(*bpm, time_signature, t)
            } else if !onsets.is_empty() {
                let base = build_time_map(*bpm, time_signature, Seconds(0.0));
                refine_beat_zero(onsets, &base, refine_threshold)
            } else {
                build_time_map(*bpm, time_signature, Seconds(0.0))
            };
            // Build per-source candidates for this fixed BPM.
            let free_candidates = offset_config.free_candidates(*bpm);
            let per_source_candidates: Vec<Vec<f64>> = pinned_offsets
                .iter()
                .map(|pin| match pin {
                    Some(n) => vec![*n],
                    None    => free_candidates.clone(),
                })
                .collect();
            let result = find_best_arrangement(
                sources,
                audio_curve,
                &time_map,
                &per_source_candidates,
                event_window_seconds,
            );
            BpmSearchResult {
                bpm: *bpm,
                beat_zero_seconds: time_map.beat_zero_seconds,
                best_offsets: result.best_offsets,
                score: result.score,
            }
        }
        BpmMode::AutoGrid { min, max, step } => {
            let estimator = AutoGridEstimator { min: *min, max: *max, step: *step };
            estimator.estimate(
                onsets,
                sources,
                audio_curve,
                time_signature,
                pinned_offsets,
                offset_config,
                event_window_seconds,
                refine_threshold,
                trim_beat_zero,
            )
        }
    }
}
