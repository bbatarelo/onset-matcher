mod domain;
mod pipeline;
mod render;

use std::path::PathBuf;
use anyhow::{Context, Result};
use clap::Parser;

use domain::time::{Seconds, TimeSignature};
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
}

// ---------------------------------------------------------------------------
// show-onsets
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
struct ShowOnsetsArgs {
    /// Path to the audio file (WAV, FLAC, MP3, OGG, etc.).
    #[arg(value_name = "AUDIO_FILE")]
    audio: PathBuf,

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

    /// Attempt to auto-refine the beat-zero position from the first strong onset.
    #[arg(long, default_value = "true")]
    refine_beat_zero: bool,

    /// Show onset strength alongside grid markers.
    #[arg(long)]
    show_strength: bool,

    /// Show the onset-strength curve as an ASCII sparkline.
    #[arg(long)]
    show_curve: bool,
}

// ---------------------------------------------------------------------------
// show-midi
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
struct ShowMidiArgs {
    /// One or more MIDI files to load and display.
    #[arg(value_name = "MIDI_FILE", required = true, num_args = 1..)]
    midi_files: Vec<PathBuf>,

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

    /// One or more MIDI files to compare against the audio.
    #[arg(value_name = "MIDI_FILE", required = true, num_args = 1..)]
    midi_files: Vec<PathBuf>,

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

    /// Attempt to auto-refine the beat-zero position from the first strong onset.
    #[arg(long, default_value = "true")]
    refine_beat_zero: bool,

    /// Show onset strength alongside audio grid markers.
    #[arg(long)]
    show_strength: bool,

    /// Show the onset-strength curve as an ASCII sparkline before the grid.
    #[arg(long)]
    show_curve: bool,
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
    }
}

// ---------------------------------------------------------------------------
// show-onsets implementation
// ---------------------------------------------------------------------------

fn run_show_onsets(args: ShowOnsetsArgs) -> Result<()> {
    if args.bpm <= 0.0 {
        anyhow::bail!("BPM must be greater than 0");
    }
    if args.subdivision <= 0.0 || args.subdivision > 4.0 {
        anyhow::bail!("subdivision must be between 0.0 and 4.0");
    }

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

    let time_sig = TimeSignature::new(args.time_sig_num, args.time_sig_den);
    let mut time_map = build_time_map(args.bpm, time_sig, Seconds(0.0));

    if args.refine_beat_zero && !analysis.observed_onsets.is_empty() {
        time_map = refine_beat_zero(&analysis.observed_onsets, &time_map, 0.3);
        println!("  Beat-zero refined to: {:.4}s", time_map.beat_zero_seconds.0);
    }

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

    let paths: Vec<&std::path::Path> = args.midi_files.iter().map(|p| p.as_path()).collect();

    println!("Loading {} MIDI file(s)...", paths.len());
    for p in &paths {
        println!("  {}", p.display());
    }

    let (sources, effective_bpm) = load_midi_sources(&paths, args.bpm)
        .context("Failed to load MIDI files")?;

    println!("  Effective BPM: {:.1}", effective_bpm);

    let time_sig = TimeSignature::new(args.time_sig_num, args.time_sig_den);
    let time_map = build_time_map(effective_bpm, time_sig, Seconds(0.0));

    let render_config = ConsoleRendererConfig {
        subdivision: args.subdivision,
        bars_per_row: args.bars_per_row,
        show_strength: false,
    };

    render_midi_sources(&sources, &time_map, &render_config);
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
    let midi_paths: Vec<&std::path::Path> = args.midi_files.iter().map(|p| p.as_path()).collect();
    println!("Loading {} MIDI file(s)...", midi_paths.len());
    for p in &midi_paths {
        println!("  {}", p.display());
    }
    let (sources, effective_bpm) = load_midi_sources(&midi_paths, args.bpm)
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

    if args.refine_beat_zero && !analysis.observed_onsets.is_empty() {
        time_map = refine_beat_zero(&analysis.observed_onsets, &time_map, 0.3);
        println!("  Beat-zero refined to: {:.4}s", time_map.beat_zero_seconds.0);
    }

    let grid_onsets = map_onsets_to_grid(&analysis.observed_onsets, &time_map, args.subdivision);

    if args.show_curve {
        render_onset_curve_sparkline(&analysis.onset_curve.values, 80);
    }

    let render_config = ConsoleRendererConfig {
        subdivision: args.subdivision,
        bars_per_row: args.bars_per_row,
        show_strength: args.show_strength,
    };

    render_compare(&grid_onsets, duration.0, &sources, &time_map, &render_config);
    Ok(())
}
