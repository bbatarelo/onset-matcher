use std::path::PathBuf;
use crate::domain::expected::ExpectedOnsetCluster;
use crate::domain::midi::MidiSource;
use crate::domain::time::{Beat, TimeMap};

/// One expected onset cluster paired with the nearest observed onset (if any).
#[derive(Debug, Clone)]
pub struct MatchedOnsetCluster {
    /// The expected cluster (beat position + event indices + expected strength).
    pub cluster: ExpectedOnsetCluster,
    /// Beat position of the matched observed onset.
    /// `None` if no observed onset was close enough.
    pub observed_beat: Option<Beat>,
    /// `observed_beat - cluster.beat`, in beats.
    /// 0.0 when unmatched.
    pub timing_error_beats: f64,
}

impl MatchedOnsetCluster {
    /// True if this cluster was matched to an observed onset.
    pub fn is_matched(&self) -> bool {
        self.observed_beat.is_some()
    }
}

/// Named warning conditions surfaced during diagnostics.
#[derive(Debug, Clone)]
pub enum AlignmentWarning {
    /// Fewer than 80% of expected clusters were matched.
    LowCoverage { matched: usize, total: usize },
    /// Mean timing error exceeds 0.1 beats.
    HighTimingError { mean_error_beats: f64 },
    /// Unexplained audio onsets (no expected cluster nearby).
    UnexplainedOnsets { count: usize },
}

/// Summary quality metrics for the best arrangement match.
#[derive(Debug, Clone)]
pub struct AlignmentDiagnostics {
    /// Total number of expected onset clusters.
    pub total_clusters: usize,
    /// Number of clusters matched to an observed onset.
    pub matched_clusters: usize,
    /// `matched_clusters / total_clusters` — [0, 1].
    pub coverage: f64,
    /// Mean absolute timing error of matched clusters, in beats.
    pub mean_timing_error_beats: f64,
    /// Beat positions of observed onsets that had no matching expected cluster.
    pub unmatched_observed_beats: Vec<Beat>,
    /// Named warnings for low-quality alignments.
    pub warnings: Vec<AlignmentWarning>,
    /// Per-cluster match details (matched and unmatched).
    pub matched_onset_clusters: Vec<MatchedOnsetCluster>,
}

/// One complete candidate arrangement with its search score and diagnostics.
#[derive(Debug, Clone)]
pub struct ArrangementMatch {
    /// MIDI sources used in this arrangement.
    pub sources: Vec<MidiSource>,
    /// Beat offset for each source (parallel to `sources`).
    pub best_offsets: Vec<f64>,
    /// Time map (BPM + beat-zero) for this arrangement.
    pub time_map: TimeMap,
    /// Raw dot-product overlap score from the search (higher = better).
    /// Not normalized; used for ranking candidates.
    pub overlap_score: f64,
    /// Quality diagnostics computed after the search.
    pub diagnostics: AlignmentDiagnostics,
}

/// Top-level result of a complete alignment run.
#[derive(Debug, Clone)]
pub struct AlignmentResult {
    /// The best-scoring arrangement.
    pub best: ArrangementMatch,
    /// Path to the reference audio file that was analysed.
    pub audio_path: PathBuf,
}
