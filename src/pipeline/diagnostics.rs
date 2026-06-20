/// Cluster-matching diagnostics.
///
/// Computes quality metrics by matching expected onset clusters (derived from
/// the MIDI arrangement) against observed audio onsets (in beat space).
///
/// This logic was previously inline in `run_score_arrangement` and is now
/// shared between `score-arrangement` and the canonical export path.
use std::collections::BTreeMap;

use crate::domain::audio::GridOnset;
use crate::domain::expected::ExpectedOnsetCluster;
use crate::domain::midi::MidiSource;
use crate::domain::result::{
    AlignmentDiagnostics, AlignmentWarning, MatchedOnsetCluster,
};
use crate::domain::time::{Beat, TimeMap};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute diagnostics for an arrangement against observed audio onsets.
///
/// # Parameters
/// - `sources`: MIDI source files in the arrangement.
/// - `offsets`: Beat offset for each source (parallel to `sources`).
/// - `grid_onsets`: Observed audio onsets mapped to beat space.
/// - `time_map`: Time map used for the arrangement.
/// - `subdivision`: Grid resolution in beats (e.g. 0.25 = 16th notes).
/// - `match_tolerance`: Maximum beat distance for a cluster to count as matched.
///
/// # Returns
/// [`AlignmentDiagnostics`] with coverage, mean timing error, unmatched onset
/// list, per-cluster match details, and named warnings.
pub fn compute_diagnostics(
    sources: &[MidiSource],
    offsets: &[f64],
    grid_onsets: &[GridOnset],
    time_map: &TimeMap,
    subdivision: f64,
    match_tolerance: f64,
) -> AlignmentDiagnostics {
    let clusters = build_clusters(sources, offsets, time_map, subdivision);
    let matched = match_clusters_to_onsets(clusters, grid_onsets, match_tolerance);

    let total = matched.len();
    let n_matched = matched.iter().filter(|c| c.is_matched()).count();

    let coverage = if total > 0 { n_matched as f64 / total as f64 } else { 0.0 };

    let mean_timing_error = if n_matched > 0 {
        matched
            .iter()
            .filter(|c| c.is_matched())
            .map(|c| c.timing_error_beats.abs())
            .sum::<f64>()
            / n_matched as f64
    } else {
        0.0
    };

    // Observed onsets that have no matched expected cluster nearby.
    let mut unmatched_observed: Vec<Beat> = grid_onsets
        .iter()
        .filter(|onset| {
            !matched.iter().any(|c| {
                c.is_matched()
                    && (c.cluster.beat.0 - onset.beat.0).abs() <= match_tolerance
            })
        })
        .map(|o| o.beat)
        .collect();
    unmatched_observed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut warnings: Vec<AlignmentWarning> = Vec::new();

    if total > 0 && coverage < 0.8 {
        warnings.push(AlignmentWarning::LowCoverage { matched: n_matched, total });
    }
    if mean_timing_error > 0.1 {
        warnings.push(AlignmentWarning::HighTimingError { mean_error_beats: mean_timing_error });
    }
    if !unmatched_observed.is_empty() {
        warnings.push(AlignmentWarning::UnexplainedOnsets { count: unmatched_observed.len() });
    }

    AlignmentDiagnostics {
        total_clusters: total,
        matched_clusters: n_matched,
        coverage,
        mean_timing_error_beats: mean_timing_error,
        unmatched_observed_beats: unmatched_observed,
        warnings,
        matched_onset_clusters: matched,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Group MIDI note-on events into subdivision-cell clusters.
///
/// Events landing in the same `(bar, sub_cell)` cell are grouped into one
/// [`ExpectedOnsetCluster`].  Returns clusters sorted by beat position.
fn build_clusters(
    sources: &[MidiSource],
    offsets: &[f64],
    time_map: &TimeMap,
    subdivision: f64,
) -> Vec<ExpectedOnsetCluster> {
    let beats_per_bar = time_map.time_signature.beats_per_bar();
    let subdivisions_per_bar = (beats_per_bar / subdivision).round() as usize;

    // Map: (bar, sub_cell) → event indices (into the flat event enumeration below)
    // We also track which *events* are in each cell so we can populate event_indices.
    // For diagnostics purposes we only need the beat and the event count; the full
    // event list is built by canonical_builder.  Here we store dummy indices 0..n.
    let mut cell_events: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    let mut event_counter: usize = 0;

    for (src_idx, source) in sources.iter().enumerate() {
        let offset = offsets.get(src_idx).copied().unwrap_or(0.0);
        for event in &source.events {
            let global_beat = event.beat.0 + offset;
            if global_beat < 0.0 {
                event_counter += 1;
                continue;
            }
            let bar = (global_beat / beats_per_bar).floor() as usize;
            let beat_in_bar = global_beat % beats_per_bar;
            let sub_idx = (beat_in_bar / subdivision).round() as usize;
            let sub_idx = sub_idx.min(subdivisions_per_bar.saturating_sub(1));
            cell_events.entry((bar, sub_idx)).or_default().push(event_counter);
            event_counter += 1;
        }
    }

    cell_events
        .into_iter()
        .map(|((bar, sub_idx), indices)| {
            let beat_val = bar as f64 * beats_per_bar + sub_idx as f64 * subdivision;
            ExpectedOnsetCluster {
                beat: Beat(beat_val),
                event_indices: indices,
                expected_strength: 1.0, // strength is not used in diagnostics
            }
        })
        .collect()
}

/// Match each cluster to the nearest observed onset within `tolerance` beats.
fn match_clusters_to_onsets(
    clusters: Vec<ExpectedOnsetCluster>,
    grid_onsets: &[GridOnset],
    tolerance: f64,
) -> Vec<MatchedOnsetCluster> {
    clusters
        .into_iter()
        .map(|cluster| {
            let best = grid_onsets.iter().min_by(|a, b| {
                let da = (a.beat.0 - cluster.beat.0).abs();
                let db = (b.beat.0 - cluster.beat.0).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });

            if let Some(onset) = best {
                let err = onset.beat.0 - cluster.beat.0;
                if err.abs() <= tolerance {
                    return MatchedOnsetCluster {
                        cluster,
                        observed_beat: Some(onset.beat),
                        timing_error_beats: err,
                    };
                }
            }

            MatchedOnsetCluster {
                cluster,
                observed_beat: None,
                timing_error_beats: 0.0,
            }
        })
        .collect()
}
