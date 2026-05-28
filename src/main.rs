mod domain;
mod pipeline;
mod render;

use std::path::PathBuf;
use anyhow::{Context, Result};
use clap::Parser;

use domain::audio::ObservedOnset;
use domain::time::{Seconds, TimeMap, TimeSignature};
use pipeline::audio_analysis::analyze_audio;
use pipeline::beat_mapper::{build_time_map, map_onsets_to_grid, refine_beat_zero};
use pipeline::midi_loader::load_midi_sources;
use pipeline::onset_detection::{PeakPickerConfig, detect_onsets};
use render::console::{ConsoleRendererConfig, render_compare, render_midi_sources, render_onset_curve_sparkline, render_onsets};

/// onset-matcher: MIDI-guided reference-audio alignment and arrangement inference tool.
#[derive(Parser, Debug)]
#[command(name = "onset-matcher", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Detect and display audio onsets as a beat grid in the terminal.
    ShowOnsets(ShowOnsetsArgs),
    /// Load and display MIDI file(s) as a beat grid in the terminal.
    ShowMidi(ShowMidiArgs),
    /// Display audio onsets and MIDI notes together on the same beat grid.
    Compare(CompareArgs),
    /// Score how well a user-specified MIDI arrangement explains the reference audio.
    ScoreArrangement(ScoreArrangementArgs),
}

// ---------------------------------------------------------------------------
// show-onsets
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
struct ShowOnsetsArgs {
    /// Path to the audio file (WAV, FLAC, MP3, OGG, etc.).
    /// Consistent with other subcommands; required at runtime.
    #[arg(long, value_name = "AUDIO_FILE")]
    audio: Option<PathBuf>,

    /// Tempo in beats per minute.
    #[arg(long, value_name = "BPM")]
    bpm: f64,

    /// Time signature numerator (beats per bar). Default: 4.
    #[arg(long, value_name = "NUMERATOR", default_value = "4")]
    time_sig_num: u8,

    /// Time signature denominator (note value of one beat). Default: 4.
    #[arg(long, value_name = "DENOMINATOR", default_value = "4")]
    time_sig_den: u8,

    /// Beat subdivision for the grid display.
    /// 0.25 = 16th notes, 0.5 = 8th notes, 1.0 = quarter notes. Default: 0.25.
    #[arg(long, value_name = "SUBDIVISION", default_value = "0.25")]
    subdivision: f64,

    /// Number of bars to display per row. Default: 4.
    #[arg(long, value_name = "N", default_value = "4")]
    bars_per_row: usize,

    /// Onset detection threshold (0.0–1.0). Lower = more sensitive. Default: 0.15.
    #[arg(long, value_name = "THRESHOLD", default_value = "0.15")]
    threshold: f32,

    /// Attempt to auto-refine the beat-zero position by snapping the first strong onset
    /// to the nearest integer beat. Uses --threshold as the strength filter.
    /// Mutually exclusive with --trim-audio.
    #[arg(long, default_value = "true", conflicts_with = "trim_audio")]
    refine_beat_zero: bool,

    /// Place the first detected onset (above --threshold) at beat 0 / bar 1 beat 1.
    /// Overrides --refine-beat-zero. Mutually exclusive with --refine-beat-zero.
    #[arg(long, conflicts_with = "refine_beat_zero")]
    trim_audio: bool,

    /// Show onset strength alongside grid markers.
    #[arg(long)]
    show_strength: bool,

    /// Show the onset-strength curve as an ASCII sparkline.
    #[arg(long)]
    show_curve: bool,
}

// ---------------------------------------------------------------------------
// Shared MIDI file specification: --midi-file / -m path[=beat_offset]
// ---------------------------------------------------------------------------

/// Parse a `--midi-file` value of the form `<path>` or `<path>=<beat_offset>`.
///
/// Returns `(path, beat_offset)`. The offset defaults to `0.0` if no `=` is present.
/// The split is on the **last** `=` so Windows paths like `C:\foo\bar.mid=8` work.
fn parse_midi_file_spec(s: &str) -> Result<(PathBuf, f64)> {
    if let Some(eq_pos) = s.rfind('=') {
        let path_str = &s[..eq_pos];
        let offset_str = &s[eq_pos + 1..];
        let offset: f64 = offset_str
            .parse()
            .with_context(|| format!("Invalid beat offset '{}' in --midi-file spec '{}'", offset_str, s))?;
        Ok((PathBuf::from(path_str), offset))
    } else {
        Ok((PathBuf::from(s), 0.0))
    }
}

/// Parse all `--midi-file` specs and return separate `(paths, offsets)` vecs.
fn parse_midi_specs(specs: &[String]) -> Result<(Vec<PathBuf>, Vec<f64>)> {
    let mut paths = Vec::with_capacity(specs.len());
    let mut offsets = Vec::with_capacity(specs.len());
    for s in specs {
        let (p, o) = parse_midi_file_spec(s)?;
        paths.push(p);
        offsets.push(o);
    }
    Ok((paths, offsets))
}

// ---------------------------------------------------------------------------
// show-midi
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
struct ShowMidiArgs {
    /// MIDI file(s) to display.  Format: <path> or <path>=<beat_offset>.
    /// Beat offset is in beats from the global timeline start (default 0).
    /// The same file may be specified multiple times with different offsets.
    /// Example: -m intro.mid=0 -m part1.mid=8
    #[arg(short = 'm', long = "midi-file", value_name = "PATH[=BEAT]", required = true)]
    midi_file: Vec<String>,

    /// Tempo in BPM. If omitted, the BPM embedded in the MIDI file is used.
    /// Required if no MIDI file contains a tempo event.
    #[arg(long, value_name = "BPM")]
    bpm: Option<f64>,

    /// Time signature numerator (beats per bar). Default: 4.
    #[arg(long, value_name = "NUMERATOR", default_value = "4")]
    time_sig_num: u8,

    /// Time signature denominator. Default: 4.
    #[arg(long, value_name = "DENOMINATOR", default_value = "4")]
    time_sig_den: u8,

    /// Beat subdivision for the grid display. Default: 0.25 (16th notes).
    #[arg(long, value_name = "SUBDIVISION", default_value = "0.25")]
    subdivision: f64,

    /// Number of bars to display per row. Default: 4.
    #[arg(long, value_name = "N", default_value = "4")]
    bars_per_row: usize,
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
struct CompareArgs {
    /// Path to the reference audio file (WAV, FLAC, MP3, OGG, etc.).
    #[arg(long, value_name = "AUDIO_FILE")]
    audio: PathBuf,

    /// MIDI file(s) to compare against the audio.  Format: <path> or <path>=<beat_offset>.
    /// Beat offset is in beats from the global timeline start (default 0).
    /// The same file may be specified multiple times with different offsets.
    /// Example: -m intro.mid=0 -m part1.mid=8
    #[arg(short = 'm', long = "midi-file", value_name = "PATH[=BEAT]", required = true)]
    midi_file: Vec<String>,

    /// Tempo in BPM. Required if no MIDI file has an embedded tempo track.
    #[arg(long, value_name = "BPM")]
    bpm: Option<f64>,

    /// Time signature numerator (beats per bar). Default: 4.
    #[arg(long, value_name = "NUMERATOR", default_value = "4")]
    time_sig_num: u8,

    /// Time signature denominator. Default: 4.
    #[arg(long, value_name = "DENOMINATOR", default_value = "4")]
    time_sig_den: u8,

    /// Beat subdivision for the grid display. Default: 0.25 (16th notes).
    #[arg(long, value_name = "SUBDIVISION", default_value = "0.25")]
    subdivision: f64,

    /// Number of bars to display per row. Default: 4.
    #[arg(long, value_name = "N", default_value = "4")]
    bars_per_row: usize,

    /// Onset detection threshold (0.0–1.0). Lower = more sensitive. Default: 0.15.
    #[arg(long, value_name = "THRESHOLD", default_value = "0.15")]
    threshold: f32,

    /// Attempt to auto-refine the beat-zero position by snapping the first strong onset
    /// to the nearest integer beat. Uses --threshold as the strength filter.
    /// Mutually exclusive with --trim-audio.
    #[arg(long, default_value = "true", conflicts_with = "trim_audio")]
    refine_beat_zero: bool,

    /// Place the first detected onset (above --threshold) at beat 0 / bar 1 beat 1.
    /// Overrides --refine-beat-zero. Mutually exclusive with --refine-beat-zero.
    #[arg(long, conflicts_with = "refine_beat_zero")]
    trim_audio: bool,

    /// Show onset strength alongside audio grid markers.
    #[arg(long)]
    show_strength: bool,

    /// Show the onset-strength curve as an ASCII sparkline before the grid.
    #[arg(long)]
    show_curve: bool,
}

// ---------------------------------------------------------------------------
// score-arrangement
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
struct ScoreArrangementArgs {
    /// Path to the reference audio file.
    #[arg(long, value_name = "AUDIO_FILE")]
    audio: PathBuf,

    /// MIDI file(s) with explicit beat placement.  Format: <path>=<beat_offset>.
    /// Beat offset is required for score-arrangement (use =0 to start at beat 0).
    /// The same file may appear multiple times at different offsets.
    /// Example: -m intro.mid=0 -m part1.mid=8 -m part1.mid=16
    #[arg(short = 'm', long = "midi-file", value_name = "PATH[=BEAT]", required = true)]
    midi_file: Vec<String>,

    /// Tempo in BPM. Required if no MIDI file has an embedded tempo track.
    #[arg(long, value_name = "BPM")]
    bpm: Option<f64>,

    /// Time signature numerator (beats per bar). Default: 4.
    #[arg(long, value_name = "NUMERATOR", default_value = "4")]
    time_sig_num: u8,

    /// Time signature denominator. Default: 4.
    #[arg(long, value_name = "DENOMINATOR", default_value = "4")]
    time_sig_den: u8,

    /// Beat subdivision for the grid display. Default: 0.25 (16th notes).
    #[arg(long, value_name = "SUBDIVISION", default_value = "0.25")]
    subdivision: f64,

    /// Number of bars to display per row. Default: 4.
    #[arg(long, value_name = "N", default_value = "4")]
    bars_per_row: usize,

    /// Onset detection threshold (0.0–1.0). Default: 0.15.
    #[arg(long, value_name = "THRESHOLD", default_value = "0.15")]
    threshold: f32,

    /// Attempt to auto-refine the beat-zero position by snapping the first strong onset
    /// to the nearest integer beat. Uses --threshold as the strength filter.
    /// Mutually exclusive with --trim-audio.
    #[arg(long, default_value = "true", conflicts_with = "trim_audio")]
    refine_beat_zero: bool,

    /// Place the first detected onset (above --threshold) at beat 0 / bar 1 beat 1.
    /// Overrides --refine-beat-zero. Mutually exclusive with --refine-beat-zero.
    #[arg(long, conflicts_with = "refine_beat_zero")]
    trim_audio: bool,

    /// Show onset strength alongside audio grid markers.
    #[arg(long)]
    show_strength: bool,

    /// Show the onset-strength curve as an ASCII sparkline before the grid.
    #[arg(long)]
    show_curve: bool,

    /// Beat-matching tolerance: an observed onset within this many beats of an
    /// expected cluster beat counts as a match. Default: 0.5 (half a beat).
    #[arg(long, value_name = "BEATS", default_value = "0.5")]
    match_tolerance: f64,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::ShowOnsets(args) => run_show_onsets(args),
        Command::ShowMidi(args) => run_show_midi(args),
        Command::Compare(args) => run_compare(args),
        Command::ScoreArrangement(args) => run_score_arrangement(args),
    }
}

// ---------------------------------------------------------------------------
// Beat-zero mode helper (shared by all audio subcommands)
// ---------------------------------------------------------------------------

/// Apply the user's beat-zero strategy and return the (possibly updated) [`TimeMap`].
///
/// Strategy priority:
/// 1. `trim_audio` — place the first onset above `threshold` exactly at beat 0.
/// 2. `refine` — snap the first beat to the nearest integer beat (default).
/// 3. Neither — return the time map unchanged.
fn apply_beat_zero_mode(
    time_map: TimeMap,
    onsets: &[ObservedOnset],
    refine: bool,
    trim: bool,
    threshold: f32,
) -> TimeMap {
    if trim {
        if let Some(first) = onsets.iter().find(|o| o.strength >= threshold) {
            return TimeMap::new(time_map.bpm, time_map.time_signature, first.time_seconds);
        }
    } else if refine && !onsets.is_empty() {
        return refine_beat_zero(onsets, &time_map, threshold);
    }
    time_map
}

// ---------------------------------------------------------------------------
// show-onsets implementation
// ---------------------------------------------------------------------------

fn run_show_onsets(args: ShowOnsetsArgs) -> Result<()> {
    let audio_path = args.audio
        .ok_or_else(|| anyhow::anyhow!("--audio <AUDIO_FILE> is required for show-onsets"))?;

    if args.bpm <= 0.0 {
        anyhow::bail!("BPM must be greater than 0");
    }
    if args.subdivision <= 0.0 || args.subdivision > 4.0 {
        anyhow::bail!("subdivision must be between 0.0 and 4.0");
    }

    println!("Loading audio: {}", audio_path.display());

    let mut analysis = analyze_audio(&audio_path)
        .with_context(|| format!("Failed to analyse audio file: {}", audio_path.display()))?;

    let duration = analysis.onset_curve.duration_seconds();
    println!(
        "  Sample rate: {} Hz  |  Duration: {:.2}s  |  {} onset frames",
        analysis.sample_rate, duration.0, analysis.onset_curve.values.len(),
    );

    let picker_config = PeakPickerConfig { threshold: args.threshold, ..Default::default() };
    let onsets = detect_onsets(&analysis.onset_curve, &picker_config);
    analysis.observed_onsets = onsets;
    println!("  Detected {} onset peaks (threshold: {})", analysis.observed_onsets.len(), args.threshold);

    let time_sig = TimeSignature::new(args.time_sig_num, args.time_sig_den);
    let mut time_map = build_time_map(args.bpm, time_sig, Seconds(0.0));

    time_map = apply_beat_zero_mode(time_map, &analysis.observed_onsets, args.refine_beat_zero, args.trim_audio, args.threshold);
    println!("  Beat-zero: {:.4}s", time_map.beat_zero_seconds.0);

    let grid_onsets = map_onsets_to_grid(&analysis.observed_onsets, &time_map, args.subdivision);

    if args.show_curve {
        render_onset_curve_sparkline(&analysis.onset_curve.values, 80);
    }

    let render_config = ConsoleRendererConfig {
        subdivision: args.subdivision,
        bars_per_row: args.bars_per_row,
        show_strength: args.show_strength,
    };

    render_onsets(&grid_onsets, &time_map, duration.0, &render_config);
    Ok(())
}

// ---------------------------------------------------------------------------
// show-midi implementation
// ---------------------------------------------------------------------------

fn run_show_midi(args: ShowMidiArgs) -> Result<()> {
    if let Some(bpm) = args.bpm {
        if bpm <= 0.0 {
            anyhow::bail!("BPM must be greater than 0");
        }
    }
    if args.subdivision <= 0.0 || args.subdivision > 4.0 {
        anyhow::bail!("subdivision must be between 0.0 and 4.0");
    }

    let (paths, offsets) = parse_midi_specs(&args.midi_file)?;
    let path_refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();

    println!("Loading {} MIDI file(s)...", path_refs.len());
    for (p, o) in paths.iter().zip(offsets.iter()) {
        if *o == 0.0 {
            println!("  {}", p.display());
        } else {
            println!("  {} (offset: {} beats)", p.display(), o);
        }
    }

    let (sources, effective_bpm) = load_midi_sources(&path_refs, args.bpm)
        .context("Failed to load MIDI files")?;

    println!("  Effective BPM: {:.1}", effective_bpm);

    let time_sig = TimeSignature::new(args.time_sig_num, args.time_sig_den);
    let time_map = build_time_map(effective_bpm, time_sig, Seconds(0.0));

    let render_config = ConsoleRendererConfig {
        subdivision: args.subdivision,
        bars_per_row: args.bars_per_row,
        show_strength: false,
    };

    render_midi_sources(&sources, &offsets, &time_map, &render_config);
    Ok(())
}

// ---------------------------------------------------------------------------
// compare implementation
// ---------------------------------------------------------------------------

fn run_compare(args: CompareArgs) -> Result<()> {
    if let Some(bpm) = args.bpm {
        if bpm <= 0.0 {
            anyhow::bail!("BPM must be greater than 0");
        }
    }
    if args.subdivision <= 0.0 || args.subdivision > 4.0 {
        anyhow::bail!("subdivision must be between 0.0 and 4.0");
    }

    // --- Load MIDI sources ---
    let (midi_paths, offsets) = parse_midi_specs(&args.midi_file)?;
    let midi_path_refs: Vec<&std::path::Path> = midi_paths.iter().map(|p| p.as_path()).collect();
    println!("Loading {} MIDI file(s)...", midi_path_refs.len());
    for (p, o) in midi_paths.iter().zip(offsets.iter()) {
        if *o == 0.0 {
            println!("  {}", p.display());
        } else {
            println!("  {} (offset: {} beats)", p.display(), o);
        }
    }
    let (sources, effective_bpm) = load_midi_sources(&midi_path_refs, args.bpm)
        .context("Failed to load MIDI files")?;
    println!("  Effective BPM: {:.1}", effective_bpm);

    // --- Load and analyse audio ---
    println!("Loading audio: {}", args.audio.display());
    let mut analysis = analyze_audio(&args.audio)
        .with_context(|| format!("Failed to analyse audio file: {}", args.audio.display()))?;

    let duration = analysis.onset_curve.duration_seconds();
    println!(
        "  Sample rate: {} Hz  |  Duration: {:.2}s  |  {} onset frames",
        analysis.sample_rate, duration.0, analysis.onset_curve.values.len(),
    );

    let picker_config = PeakPickerConfig { threshold: args.threshold, ..Default::default() };
    let onsets = detect_onsets(&analysis.onset_curve, &picker_config);
    analysis.observed_onsets = onsets;
    println!("  Detected {} onset peaks (threshold: {})", analysis.observed_onsets.len(), args.threshold);

    // --- Build TimeMap ---
    let time_sig = TimeSignature::new(args.time_sig_num, args.time_sig_den);
    let mut time_map = build_time_map(effective_bpm, time_sig, Seconds(0.0));

    time_map = apply_beat_zero_mode(time_map, &analysis.observed_onsets, args.refine_beat_zero, args.trim_audio, args.threshold);
    println!("  Beat-zero: {:.4}s", time_map.beat_zero_seconds.0);

    let grid_onsets = map_onsets_to_grid(&analysis.observed_onsets, &time_map, args.subdivision);

    if args.show_curve {
        render_onset_curve_sparkline(&analysis.onset_curve.values, 80);
    }

    let render_config = ConsoleRendererConfig {
        subdivision: args.subdivision,
        bars_per_row: args.bars_per_row,
        show_strength: args.show_strength,
    };

    render_compare(&grid_onsets, duration.0, &sources, &offsets, &time_map, &render_config);
    Ok(())
}

// ---------------------------------------------------------------------------
// score-arrangement implementation
// ---------------------------------------------------------------------------

fn run_score_arrangement(args: ScoreArrangementArgs) -> Result<()> {
    if let Some(bpm) = args.bpm {
        if bpm <= 0.0 {
            anyhow::bail!("BPM must be greater than 0");
        }
    }
    if args.subdivision <= 0.0 || args.subdivision > 4.0 {
        anyhow::bail!("subdivision must be between 0.0 and 4.0");
    }
    if args.match_tolerance <= 0.0 {
        anyhow::bail!("match-tolerance must be greater than 0");
    }

    // --- Load MIDI sources ---
    let (midi_paths, offsets) = parse_midi_specs(&args.midi_file)?;
    let midi_path_refs: Vec<&std::path::Path> = midi_paths.iter().map(|p| p.as_path()).collect();
    println!("Loading {} MIDI file(s)...", midi_path_refs.len());
    for (p, o) in midi_paths.iter().zip(offsets.iter()) {
        println!("  {} (offset: {} beats)", p.display(), o);
    }
    let (sources, effective_bpm) = load_midi_sources(&midi_path_refs, args.bpm)
        .context("Failed to load MIDI files")?;
    println!("  Effective BPM: {:.1}", effective_bpm);

    // --- Load and analyse audio ---
    println!("Loading audio: {}", args.audio.display());
    let mut analysis = analyze_audio(&args.audio)
        .with_context(|| format!("Failed to analyse audio file: {}", args.audio.display()))?;

    let duration = analysis.onset_curve.duration_seconds();
    println!(
        "  Sample rate: {} Hz  |  Duration: {:.2}s  |  {} onset frames",
        analysis.sample_rate, duration.0, analysis.onset_curve.values.len(),
    );

    let picker_config = PeakPickerConfig { threshold: args.threshold, ..Default::default() };
    let onsets = detect_onsets(&analysis.onset_curve, &picker_config);
    analysis.observed_onsets = onsets;
    println!("  Detected {} onset peaks (threshold: {})", analysis.observed_onsets.len(), args.threshold);

    // --- Build TimeMap ---
    let time_sig = TimeSignature::new(args.time_sig_num, args.time_sig_den);
    let mut time_map = build_time_map(effective_bpm, time_sig, Seconds(0.0));

    time_map = apply_beat_zero_mode(time_map, &analysis.observed_onsets, args.refine_beat_zero, args.trim_audio, args.threshold);
    println!("  Beat-zero: {:.4}s", time_map.beat_zero_seconds.0);

    let grid_onsets = map_onsets_to_grid(&analysis.observed_onsets, &time_map, args.subdivision);

    if args.show_curve {
        render_onset_curve_sparkline(&analysis.onset_curve.values, 80);
    }

    // --- Render grid (same as compare) ---
    let render_config = ConsoleRendererConfig {
        subdivision: args.subdivision,
        bars_per_row: args.bars_per_row,
        show_strength: args.show_strength,
    };
    render_compare(&grid_onsets, duration.0, &sources, &offsets, &time_map, &render_config);

    // --- Build expected onset clusters from arrangement ---
    // Collect all MIDI note-on events with their global beat (source beat + layer offset).
    // Group events that land in the same subdivision cell into clusters.
    let beats_per_bar = time_map.time_signature.beats_per_bar();
    let subdivisions_per_bar = (beats_per_bar / args.subdivision).round() as usize;

    // Map: grid cell (bar, sub_idx) -> Vec<(note, source_idx)>
    let mut cell_events: std::collections::BTreeMap<(usize, usize), Vec<u8>> =
        std::collections::BTreeMap::new();
    for (src_idx, source) in sources.iter().enumerate() {
        let offset = offsets.get(src_idx).copied().unwrap_or(0.0);
        for event in &source.events {
            let global_beat = event.beat.0 + offset;
            if global_beat < 0.0 { continue; }
            let bar = (global_beat / beats_per_bar).floor() as usize;
            let beat_in_bar = global_beat % beats_per_bar;
            let sub_idx = (beat_in_bar / args.subdivision).round() as usize;
            let sub_idx = sub_idx.min(subdivisions_per_bar - 1);
            cell_events.entry((bar, sub_idx)).or_default().push(event.note);
        }
    }

    // Each occupied cell is one expected onset cluster.
    // Representative beat = bar * beats_per_bar + sub_idx * subdivision.
    struct Cluster {
        beat: f64,
        notes: Vec<u8>,
        matched: bool,
        error_beats: f64,
    }

    let mut clusters: Vec<Cluster> = cell_events
        .into_iter()
        .map(|((bar, sub_idx), mut notes)| {
            notes.sort_unstable();
            notes.dedup();
            let beat = bar as f64 * beats_per_bar + sub_idx as f64 * args.subdivision;
            Cluster { beat, notes, matched: false, error_beats: 0.0 }
        })
        .collect();

    // Match each cluster to the nearest observed onset within tolerance.
    // Observed onsets are in beat space via grid_onsets.
    let tolerance = args.match_tolerance;
    for cluster in &mut clusters {
        // Find the observed onset closest to cluster.beat.
        let best = grid_onsets.iter().min_by(|a, b| {
            let da = (a.beat.0 - cluster.beat).abs();
            let db = (b.beat.0 - cluster.beat).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(onset) = best {
            let err = onset.beat.0 - cluster.beat;
            if err.abs() <= tolerance {
                cluster.matched = true;
                cluster.error_beats = err;
            }
        }
    }

    // --- Print scoring results ---
    let matched = clusters.iter().filter(|c| c.matched).count();
    let total = clusters.len();
    let score = if total > 0 { matched as f64 / total as f64 } else { 0.0 };

    println!();
    println!(
        "  Score: {:.3} ({:.1}%)  |  {}/{} clusters matched  |  tolerance: ±{} beats",
        score, score * 100.0, matched, total, tolerance
    );
    println!();
    println!("  Cluster analysis:");

    for cluster in &clusters {
        if cluster.matched {
            println!(
                "    Beat {:>7.3}  ■ matched   error: {:+.4} beats   notes: {:?}",
                cluster.beat, cluster.error_beats, cluster.notes
            );
        } else {
            println!(
                "    Beat {:>7.3}  ✗ unmatched                          notes: {:?}",
                cluster.beat, cluster.notes
            );
        }
    }

    // Unmatched observed onsets.
    let mut unmatched_onsets: Vec<f64> = grid_onsets
        .iter()
        .filter(|onset| {
            !clusters.iter().any(|c| c.matched && (c.beat - onset.beat.0).abs() <= tolerance)
        })
        .map(|o| o.beat.0)
        .collect();
    unmatched_onsets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if !unmatched_onsets.is_empty() {
        println!();
        println!("  Unmatched audio onsets ({}):", unmatched_onsets.len());
        for beat in &unmatched_onsets {
            println!("    Beat {:>7.3}  (no expected cluster within ±{} beats)", beat, tolerance);
        }
    }
    println!();

    Ok(())
}
