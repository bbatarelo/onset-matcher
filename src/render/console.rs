use crate::domain::audio::GridOnset;
use crate::domain::midi::MidiSource;
use crate::domain::time::TimeMap;

/// Label used for the audio onset row in the combined `render_compare` grid.
pub const AUDIO_ROW_LABEL: &str = "audio";

/// Configuration for the console onset grid renderer.
pub struct ConsoleRendererConfig {
    /// Beat subdivision to use for columns.
    /// 0.25 = 16th notes, 0.5 = 8th notes, 1.0 = quarter notes.
    pub subdivision: f64,
    /// Maximum number of bars to print per visual row before wrapping.
    pub bars_per_row: usize,
    /// Whether to show the onset strength alongside the marker.
    pub show_strength: bool,
}

impl Default for ConsoleRendererConfig {
    fn default() -> Self {
        Self {
            subdivision: 0.25, // 16th notes
            bars_per_row: 4,
            show_strength: false,
        }
    }
}

/// Render a list of `GridOnset`s as a text grid in the terminal.
///
/// Layout:
///   - Each row covers `bars_per_row` bars.
///   - Columns are beat subdivisions within a bar.
///   - An onset is shown in the cell nearest to its beat position.
///   - Strength is encoded as: `·` (< 0.33), `•` (< 0.66), `●` (≥ 0.66).
pub fn render_onsets(
    grid_onsets: &[GridOnset],
    time_map: &TimeMap,
    audio_duration_seconds: f64,
    config: &ConsoleRendererConfig,
) {
    let beats_per_bar = time_map.time_signature.beats_per_bar();
    let subdivisions_per_bar =
        (beats_per_bar / config.subdivision).round() as usize;
    let total_beats =
        (audio_duration_seconds - time_map.beat_zero_seconds.0).max(0.0)
            / time_map.seconds_per_beat();
    let total_bars = (total_beats / beats_per_bar).ceil() as usize;

    println!();
    println!(
        "  BPM: {}  |  Time signature: {}  |  {} bars  |  {} detected onsets",
        time_map.bpm,
        time_map.time_signature,
        total_bars,
        grid_onsets.len(),
    );
    println!();

    // Build a lookup: (bar, subdivision_index) -> strongest onset strength
    // (there can be multiple onsets landing in the same cell).
    let mut grid: std::collections::HashMap<(usize, usize), f32> = std::collections::HashMap::new();

    for onset in grid_onsets {
        let bar = onset.bar as usize;
        // Which subdivision column does this beat fall in?
        let sub_idx = (onset.beat_in_bar / config.subdivision).round() as usize;
        let sub_idx = sub_idx.min(subdivisions_per_bar - 1);
        let entry = grid.entry((bar, sub_idx)).or_insert(0.0);
        if onset.strength > *entry {
            *entry = onset.strength;
        }
    }

    // Print row by row.
    let rows = (total_bars + config.bars_per_row - 1) / config.bars_per_row;

    for row in 0..rows {
        let bar_start = row * config.bars_per_row;
        let bar_end = (bar_start + config.bars_per_row).min(total_bars);

        // Header: bar numbers.
        print!("  ");
        for bar in bar_start..bar_end {
            let bar_label = format!(" Bar {:>3} ", bar + 1);
            // Each bar occupies subdivisions_per_bar * 2 chars + 1 separator.
            let bar_width = subdivisions_per_bar * 2;
            let label = format!("{:^width$}", bar_label, width = bar_width);
            print!("|{}", label);
        }
        println!("|");

        // Sub-header: beat markers within each bar.
        print!("  ");
        for _ in bar_start..bar_end {
            print!("|");
            for sub in 0..subdivisions_per_bar {
                let beat_pos = sub as f64 * config.subdivision;
                // Mark beat boundaries with beat number, off-beats with dots.
                if beat_pos.fract() == 0.0 {
                    print!("{:<2}", beat_pos as usize + 1);
                } else {
                    print!(". ");
                }
            }
        }
        println!("|");

        // Onset row.
        print!("  ");
        for bar in bar_start..bar_end {
            print!("|");
            for sub in 0..subdivisions_per_bar {
                if let Some(&strength) = grid.get(&(bar, sub)) {
                    let marker = strength_to_marker(strength);
                    if config.show_strength {
                        print!("{}{:.0}", marker, strength * 9.0);
                    } else {
                        print!("{} ", marker);
                    }
                } else {
                    print!("  ");
                }
            }
        }
        println!("|");
        println!();
    }

    // Summary statistics.
    if !grid_onsets.is_empty() {
        let mean_strength: f32 =
            grid_onsets.iter().map(|o| o.strength).sum::<f32>() / grid_onsets.len() as f32;
        let mean_qe: f64 =
            grid_onsets.iter().map(|o| o.quantization_error_beats.abs()).sum::<f64>()
                / grid_onsets.len() as f64;

        println!(
            "  Mean onset strength: {:.2}  |  Mean quantization error: {:.4} beats ({:.1} ms at {} BPM)",
            mean_strength,
            mean_qe,
            mean_qe * 60.0 / time_map.bpm * 1000.0,
            time_map.bpm,
        );
        println!();
    }
}

/// Render one or more `MidiSource`s as a beat grid in the terminal.
///
/// Each MIDI source gets its own labeled row within each bar's cell.
/// A `■` marker indicates a note-on event in that subdivision.
/// If multiple notes fire in the same subdivision they still show as one `■`.
///
/// `offsets` is a per-source beat offset (same length as `sources`).
/// Pass `&[]` or a slice of zeros to start all sources at beat 0.
pub fn render_midi_sources(
    sources: &[MidiSource],
    offsets: &[f64],
    time_map: &TimeMap,
    config: &ConsoleRendererConfig,
) {
    let beats_per_bar = time_map.time_signature.beats_per_bar();
    let subdivisions_per_bar = (beats_per_bar / config.subdivision).round() as usize;

    // Find the end beat of each source (its length + its offset).
    let max_beats = sources
        .iter()
        .enumerate()
        .map(|(i, s)| s.length_beats.0 + offsets.get(i).copied().unwrap_or(0.0))
        .fold(0.0_f64, f64::max);
    let total_bars = (max_beats / beats_per_bar).ceil() as usize;

    println!();
    println!(
        "  BPM: {}  |  Time signature: {}  |  {} bars  |  {} source(s)",
        time_map.bpm,
        time_map.time_signature,
        total_bars,
        sources.len(),
    );

    // For each source, build a (bar, sub_idx) -> bool lookup applying the offset.
    let source_grids: Vec<std::collections::HashSet<(usize, usize)>> = sources
        .iter()
        .enumerate()
        .map(|(i, source)| {
            let offset = offsets.get(i).copied().unwrap_or(0.0);
            let mut grid = std::collections::HashSet::new();
            for event in &source.events {
                let global_beat = event.beat.0 + offset;
                if global_beat < 0.0 { continue; }
                let bar = (global_beat / beats_per_bar).floor() as usize;
                let beat_in_bar = global_beat % beats_per_bar;
                let sub_idx = (beat_in_bar / config.subdivision).round() as usize;
                let sub_idx = sub_idx.min(subdivisions_per_bar - 1);
                grid.insert((bar, sub_idx));
            }
            grid
        })
        .collect();

    let rows = (total_bars + config.bars_per_row - 1) / config.bars_per_row;

    for row in 0..rows {
        let bar_start = row * config.bars_per_row;
        let bar_end = (bar_start + config.bars_per_row).min(total_bars);
        let bar_width = subdivisions_per_bar * 2;

        // Header: bar numbers.
        print!("  {:12}", "");
        for bar in bar_start..bar_end {
            let bar_label = format!(" Bar {:>3} ", bar + 1);
            print!("|{}", format!("{:^width$}", bar_label, width = bar_width));
        }
        println!("|");

        // Sub-header: beat markers.
        print!("  {:12}", "");
        for _ in bar_start..bar_end {
            print!("|");
            for sub in 0..subdivisions_per_bar {
                let beat_pos = sub as f64 * config.subdivision;
                if beat_pos.fract() == 0.0 {
                    print!("{:<2}", beat_pos as usize + 1);
                } else {
                    print!(". ");
                }
            }
        }
        println!("|");

        // One event row per MIDI source.
        for (src_idx, source) in sources.iter().enumerate() {
            let label = format!("  {:12}", truncate_label(source.id.as_str(), 12));
            print!("{}", label);
            for bar in bar_start..bar_end {
                print!("|");
                for sub in 0..subdivisions_per_bar {
                    if source_grids[src_idx].contains(&(bar, sub)) {
                        print!("■ ");
                    } else {
                        print!("  ");
                    }
                }
            }
            println!("|  {}", source.id.as_str());
        }
        println!();
    }

    // Per-source summary.
    for source in sources {
        println!(
            "  {}  |  {:.2} beats  |  {} events  |  {} unique notes  |  PPQ: {}{}",
            source.id.as_str(),
            source.length_beats.0,
            source.events.len(),
            source.unique_notes().len(),
            source.ppq,
            source.embedded_bpm
                .map(|b| format!("  |  embedded BPM: {:.1}", b))
                .unwrap_or_default(),
        );
    }
    println!();
}

/// Render audio onsets and one or more MIDI sources together on the same beat grid.
///
/// Layout (per row):
///   - One header row with bar numbers
///   - One sub-header row with beat/subdivision markers
///   - One audio onset row labelled "audio" (onset strength markers: `·` `•` `●`)
///   - One row per MIDI source (note markers: `■`)
///
/// `offsets` is a per-source beat offset (same length as `sources`).
/// The grid span is the longer of the audio duration and the longest offset MIDI source.
pub fn render_compare(
    grid_onsets: &[GridOnset],
    audio_duration_seconds: f64,
    sources: &[MidiSource],
    offsets: &[f64],
    time_map: &TimeMap,
    config: &ConsoleRendererConfig,
) {
    let beats_per_bar = time_map.time_signature.beats_per_bar();
    let subdivisions_per_bar = (beats_per_bar / config.subdivision).round() as usize;

    // Total span: max of audio and MIDI (including offsets).
    let audio_beats =
        (audio_duration_seconds - time_map.beat_zero_seconds.0).max(0.0)
            / time_map.seconds_per_beat();
    let midi_beats = sources
        .iter()
        .enumerate()
        .map(|(i, s)| s.length_beats.0 + offsets.get(i).copied().unwrap_or(0.0))
        .fold(0.0_f64, f64::max);
    let total_beats = audio_beats.max(midi_beats);
    let total_bars = (total_beats / beats_per_bar).ceil() as usize;

    println!();
    println!(
        "  BPM: {}  |  Time signature: {}  |  {} bars  |  {} detected onsets  |  {} source(s)",
        time_map.bpm,
        time_map.time_signature,
        total_bars,
        grid_onsets.len(),
        sources.len(),
    );
    println!();

    // Build audio onset lookup: (bar, sub_idx) -> strongest strength.
    let mut audio_grid: std::collections::HashMap<(usize, usize), f32> =
        std::collections::HashMap::new();
    for onset in grid_onsets {
        let bar = onset.bar as usize;
        let sub_idx = (onset.beat_in_bar / config.subdivision).round() as usize;
        let sub_idx = sub_idx.min(subdivisions_per_bar - 1);
        let entry = audio_grid.entry((bar, sub_idx)).or_insert(0.0);
        if onset.strength > *entry {
            *entry = onset.strength;
        }
    }

    // Build per-source MIDI lookup applying offsets: (bar, sub_idx) -> present.
    let source_grids: Vec<std::collections::HashSet<(usize, usize)>> = sources
        .iter()
        .enumerate()
        .map(|(i, source)| {
            let offset = offsets.get(i).copied().unwrap_or(0.0);
            let mut grid = std::collections::HashSet::new();
            for event in &source.events {
                let global_beat = event.beat.0 + offset;
                if global_beat < 0.0 { continue; }
                let bar = (global_beat / beats_per_bar).floor() as usize;
                let beat_in_bar = global_beat % beats_per_bar;
                let sub_idx = (beat_in_bar / config.subdivision).round() as usize;
                let sub_idx = sub_idx.min(subdivisions_per_bar - 1);
                grid.insert((bar, sub_idx));
            }
            grid
        })
        .collect();

    let rows = (total_bars + config.bars_per_row - 1) / config.bars_per_row;
    let label_width: usize = 12;

    for row in 0..rows {
        let bar_start = row * config.bars_per_row;
        let bar_end = (bar_start + config.bars_per_row).min(total_bars);
        let bar_width = subdivisions_per_bar * 2;

        // Header: bar numbers.
        print!("  {:width$}", "", width = label_width);
        for bar in bar_start..bar_end {
            let bar_label = format!(" Bar {:>3} ", bar + 1);
            print!("|{}", format!("{:^width$}", bar_label, width = bar_width));
        }
        println!("|");

        // Sub-header: beat markers.
        print!("  {:width$}", "", width = label_width);
        for _ in bar_start..bar_end {
            print!("|");
            for sub in 0..subdivisions_per_bar {
                let beat_pos = sub as f64 * config.subdivision;
                if beat_pos.fract() == 0.0 {
                    print!("{:<2}", beat_pos as usize + 1);
                } else {
                    print!(". ");
                }
            }
        }
        println!("|");

        // Audio onset row.
        print!("  {}", format!("{:width$}", truncate_label(AUDIO_ROW_LABEL, label_width), width = label_width));
        for bar in bar_start..bar_end {
            print!("|");
            for sub in 0..subdivisions_per_bar {
                if let Some(&strength) = audio_grid.get(&(bar, sub)) {
                    let marker = strength_to_marker(strength);
                    if config.show_strength {
                        print!("{}{:.0}", marker, strength * 9.0);
                    } else {
                        print!("{} ", marker);
                    }
                } else {
                    print!("  ");
                }
            }
        }
        println!("|  (audio onsets)");

        // One row per MIDI source.
        for (src_idx, source) in sources.iter().enumerate() {
            print!("  {}", format!("{:width$}", truncate_label(source.id.as_str(), label_width), width = label_width));
            for bar in bar_start..bar_end {
                print!("|");
                for sub in 0..subdivisions_per_bar {
                    if source_grids[src_idx].contains(&(bar, sub)) {
                        print!("■ ");
                    } else {
                        print!("  ");
                    }
                }
            }
            println!("|  {}", source.id.as_str());
        }
        println!();
    }

    // Summary.
    if !grid_onsets.is_empty() {
        let mean_strength: f32 =
            grid_onsets.iter().map(|o| o.strength).sum::<f32>() / grid_onsets.len() as f32;
        let mean_qe: f64 =
            grid_onsets.iter().map(|o| o.quantization_error_beats.abs()).sum::<f64>()
                / grid_onsets.len() as f64;
        println!(
            "  Audio: {} onsets  |  mean strength: {:.2}  |  mean quantization error: {:.4} beats ({:.1} ms)",
            grid_onsets.len(),
            mean_strength,
            mean_qe,
            mean_qe * 60.0 / time_map.bpm * 1000.0,
        );
    }
    for source in sources {
        println!(
            "  {}  |  {:.2} beats  |  {} events  |  {} unique notes  |  PPQ: {}{}",
            source.id.as_str(),
            source.length_beats.0,
            source.events.len(),
            source.unique_notes().len(),
            source.ppq,
            source.embedded_bpm
                .map(|b| format!("  |  embedded BPM: {:.1}", b))
                .unwrap_or_default(),
        );
    }
    println!();
}

fn truncate_label(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}

fn strength_to_marker(strength: f32) -> char {
    if strength >= 0.66 {
        '●'
    } else if strength >= 0.33 {
        '•'
    } else {
        '·'
    }
}

/// Print the raw onset-strength curve as a small ASCII sparkline (for debugging).
pub fn render_onset_curve_sparkline(values: &[f32], width: usize) {
    const BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let step = (values.len() as f64 / width as f64).ceil() as usize;
    let step = step.max(1);

    print!("  Onset curve: |");
    for chunk_start in (0..values.len()).step_by(step) {
        let chunk_end = (chunk_start + step).min(values.len());
        let max_val = values[chunk_start..chunk_end]
            .iter()
            .cloned()
            .fold(0.0_f32, f32::max);
        let idx = ((max_val * (BLOCKS.len() - 1) as f32).round() as usize)
            .min(BLOCKS.len() - 1);
        print!("{}", BLOCKS[idx]);
    }
    println!("|");
}
