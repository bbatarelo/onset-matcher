/// Build `CanonicalPlaybackReference` and `TestFixture` from an alignment result.
///
/// `build_canonical` renders all MIDI note-on events onto the global beat
/// timeline and assembles the full rich JSON output.
///
/// `build_test_fixture` flattens the canonical event list into the minimal
/// per-beat format used by application-level tests.
use std::collections::BTreeMap;
use std::path::Path;

use crate::domain::canonical::{
    CanonicalEvent, CanonicalLayer, CanonicalMusicalContext, CanonicalOnsetCluster,
    CanonicalPlaybackReference, CanonicalSource, ReferenceProvenance, TestFixture,
    TestFixtureEvent,
};
use crate::domain::midi::MidiSource;
use crate::domain::result::AlignmentDiagnostics;
use crate::domain::time::TimeMap;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a [`CanonicalPlaybackReference`] from a completed arrangement.
///
/// # Parameters
/// - `name`: Human-readable name for this fixture (e.g. "my_drum_pattern").
/// - `sources`: MIDI source files in the arrangement.
/// - `offsets`: Beat offset for each source (parallel to `sources`).
/// - `time_map`: Time map (BPM + beat-zero) for the arrangement.
/// - `diagnostics`: Pre-computed alignment diagnostics.
/// - `audio_path`: Path to the reference audio file (for provenance metadata).
/// - `overlap_score`: Raw dot-product overlap score from the search (for internal
///   reference; not used as `matchScore` — that uses `diagnostics.coverage`).
pub fn build_canonical(
    name: &str,
    sources: &[MidiSource],
    offsets: &[f64],
    time_map: &TimeMap,
    diagnostics: &AlignmentDiagnostics,
    audio_path: &Path,
    overlap_score: f64,
) -> CanonicalPlaybackReference {
    let _ = overlap_score; // available for future use (e.g. searchScore field)

    // --- Sources ---
    let canonical_sources: Vec<CanonicalSource> = sources
        .iter()
        .map(|s| CanonicalSource {
            id: s.id.0.clone(),
            path: s.path.display().to_string(),
            length_beats: s.length_beats.0,
        })
        .collect();

    // --- Layers (one layer per source instance) ---
    let canonical_layers: Vec<CanonicalLayer> = sources
        .iter()
        .zip(offsets.iter())
        .enumerate()
        .map(|(i, (source, &offset))| {
            let layer_id = format!("layer_{}", i + 1);
            let start = offset.max(0.0);
            let end = start + source.length_beats.0;
            CanonicalLayer {
                id: layer_id,
                source_id: source.id.0.clone(),
                start_beat: start,
                end_beat: end,
                source_start_beat: 0.0,
            }
        })
        .collect();

    // --- Events: all note-on events on global beat timeline ---
    // Ordered by global beat, then by source index for ties.
    let mut events: Vec<CanonicalEvent> = Vec::new();
    for (src_idx, (source, &offset)) in sources.iter().zip(offsets.iter()).enumerate() {
        let layer_id = format!("layer_{}", src_idx + 1);
        let global_offset = offset.max(0.0);
        for event in &source.events {
            let global_beat = event.beat.0 + global_offset;
            if global_beat < 0.0 {
                continue;
            }
            events.push(CanonicalEvent {
                beat: global_beat,
                source_id: source.id.0.clone(),
                layer_id: layer_id.clone(),
                source_beat: event.beat.0,
                note: event.note,
                velocity: event.velocity,
            });
        }
    }
    // Sort by global beat (stable sort preserves source order for ties).
    events.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));

    // --- Onset clusters: one per matched-cluster entry in diagnostics ---
    // Build a lookup from cluster beat → event indices in the sorted events list.
    let onset_clusters = build_onset_clusters(&events, diagnostics, time_map);

    // --- Musical context ---
    let ppq = sources.first().map(|s| s.ppq).unwrap_or(960);
    let musical_context = CanonicalMusicalContext {
        bpm: time_map.bpm,
        time_signature: [
            time_map.time_signature.numerator,
            time_map.time_signature.denominator,
        ],
        ppq,
    };

    // --- Provenance ---
    let provenance = ReferenceProvenance {
        generated_by: format!("onset-matcher {}", env!("CARGO_PKG_VERSION")),
        source_audio: audio_path.display().to_string(),
        match_score: diagnostics.coverage,
        review_status: "auto".to_string(),
    };

    CanonicalPlaybackReference {
        schema_version: "0.1".to_string(),
        name: name.to_string(),
        musical_context,
        sources: canonical_sources,
        layers: canonical_layers,
        events,
        onset_clusters,
        provenance,
    }
}

/// Build a [`TestFixture`] by flattening a [`CanonicalPlaybackReference`].
///
/// All events are grouped by beat cluster.  Velocity is dropped; source and
/// layer IDs are dropped.  Notes within each cluster are sorted and deduplicated.
pub fn build_test_fixture(canonical: &CanonicalPlaybackReference) -> TestFixture {
    // Group events by their beat position (round to 6 decimal places to avoid
    // float equality surprises while still grouping genuinely simultaneous notes).
    let mut beat_map: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for event in &canonical.events {
        // Use a scaled integer key so we can use BTreeMap ordering.
        let key = (event.beat * 1_000_000.0).round() as u64;
        beat_map.entry(key).or_default().push(event.note);
    }

    let events: Vec<TestFixtureEvent> = beat_map
        .into_iter()
        .map(|(key, mut notes)| {
            notes.sort_unstable();
            notes.dedup();
            TestFixtureEvent {
                beat: key as f64 / 1_000_000.0,
                notes,
            }
        })
        .collect();

    TestFixture {
        schema_version: "0.1".to_string(),
        name: canonical.name.clone(),
        bpm: canonical.musical_context.bpm,
        time_signature: canonical.musical_context.time_signature,
        events,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build canonical onset clusters from the sorted event list and diagnostics.
///
/// For each cluster in diagnostics, we find the event indices in the `events`
/// list that fall at that beat position (within half a subdivision).
fn build_onset_clusters(
    events: &[CanonicalEvent],
    diagnostics: &AlignmentDiagnostics,
    _time_map: &TimeMap,
) -> Vec<CanonicalOnsetCluster> {
    // Use a small tolerance for beat-matching events to clusters.
    let eps = 0.001; // 1/1000 of a beat — tight enough not to cross-contaminate

    diagnostics
        .matched_onset_clusters
        .iter()
        .map(|mc| {
            let cluster_beat = mc.cluster.beat.0;

            // Find all event indices at this beat.
            let event_indices: Vec<usize> = events
                .iter()
                .enumerate()
                .filter(|(_, e)| (e.beat - cluster_beat).abs() <= eps)
                .map(|(i, _)| i)
                .collect();

            let confidence = if mc.is_matched() { 1.0 } else { 0.0 };

            CanonicalOnsetCluster {
                beat: cluster_beat,
                event_indices,
                expected_strength: mc.cluster.expected_strength,
                confidence,
            }
        })
        .collect::<Vec<_>>()
        .tap_sorted_by_beat()
}

// ---------------------------------------------------------------------------
// Small extension trait for post-processing the cluster list
// ---------------------------------------------------------------------------

trait SortByBeat {
    fn tap_sorted_by_beat(self) -> Self;
}

impl SortByBeat for Vec<CanonicalOnsetCluster> {
    fn tap_sorted_by_beat(mut self) -> Self {
        self.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));
        self
    }
}
