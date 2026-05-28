use std::collections::HashMap;
use std::path::Path;
use anyhow::{Context, Result, bail};
use midly::{Smf, TrackEventKind, MidiMessage, MetaMessage};

use crate::domain::midi::{MidiEvent, MidiSource, SourceId};
use crate::domain::time::Beat;

/// Load and parse a MIDI file into a `MidiSource`.
///
/// BPM resolution:
///   - If `override_bpm` is `Some(bpm)`, use that.
///   - Otherwise, use the first SetTempo meta event found in any track.
///   - If neither is available, return an error.
pub fn load_midi(path: &Path, override_bpm: Option<f64>) -> Result<MidiSource> {
    let id = SourceId::new(
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown"),
    );

    let bytes = std::fs::read(path)
        .with_context(|| format!("Cannot read MIDI file: {}", path.display()))?;

    let smf = Smf::parse(&bytes)
        .with_context(|| format!("Cannot parse MIDI file: {}", path.display()))?;

    let ppq = match smf.header.timing {
        midly::Timing::Metrical(tpb) => tpb.as_int() as u16,
        midly::Timing::Timecode(_, _) => {
            bail!("MIDI file uses SMPTE timecode timing, which is not yet supported: {}", path.display())
        }
    };

    // --- Collect tempo events and note events from all tracks ---

    // Tempo map: absolute_tick -> microseconds_per_beat
    // Used to convert absolute ticks to beats (handles tempo changes).
    let mut tempo_map: Vec<(u64, u32)> = Vec::new();
    // Collected note-on events: (absolute_tick, channel, note, velocity)
    let mut raw_note_ons: Vec<(u64, u8, u8, u8)> = Vec::new();
    // Collected note-off events: (absolute_tick, channel, note)
    let mut raw_note_offs: Vec<(u64, u8, u8)> = Vec::new();

    for track in &smf.tracks {
        let mut absolute_tick: u64 = 0;
        for event in track {
            absolute_tick += event.delta.as_int() as u64;

            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(micros)) => {
                    tempo_map.push((absolute_tick, micros.as_int()));
                }
                TrackEventKind::Midi { channel, message } => {
                    match message {
                        MidiMessage::NoteOn { key, vel } => {
                            if vel.as_int() > 0 {
                                raw_note_ons.push((absolute_tick, channel.as_int(), key.as_int(), vel.as_int()));
                            } else {
                                // NoteOn with velocity 0 = NoteOff in MIDI spec
                                raw_note_offs.push((absolute_tick, channel.as_int(), key.as_int()));
                            }
                        }
                        MidiMessage::NoteOff { key, .. } => {
                            raw_note_offs.push((absolute_tick, channel.as_int(), key.as_int()));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    // Sort tempo map by tick position (should already be sorted, but be safe).
    tempo_map.sort_by_key(|(tick, _)| *tick);

    // --- Determine effective BPM ---

    let embedded_bpm = tempo_map.first().map(|(_, micros)| 60_000_000.0 / *micros as f64);

    let effective_bpm = match override_bpm {
        Some(bpm) => bpm,
        None => embedded_bpm.ok_or_else(|| {
            anyhow::anyhow!(
                "MIDI file has no embedded tempo and --bpm was not specified: {}",
                path.display()
            )
        })?,
    };

    if effective_bpm <= 0.0 {
        bail!("BPM must be > 0, got: {}", effective_bpm);
    }

    // --- Build tick-to-beat converter ---
    // For simplicity, use the effective_bpm (ignoring mid-file tempo changes).
    // A full implementation would walk the tempo map for precise conversion.
    // TODO: support mid-file tempo changes using the tempo_map.
    let ticks_per_beat = ppq as f64;

    let tick_to_beat = |tick: u64| -> Beat {
        Beat(tick as f64 / ticks_per_beat)
    };

    // --- Build note-off lookup: (channel, note) -> sorted list of off ticks ---
    let mut note_off_map: HashMap<(u8, u8), Vec<u64>> = HashMap::new();
    for (tick, ch, note) in &raw_note_offs {
        note_off_map.entry((*ch, *note)).or_default().push(*tick);
    }
    // Sort each list so we can binary-search for the first off after an on.
    for offs in note_off_map.values_mut() {
        offs.sort_unstable();
    }

    // --- Convert raw note-ons to MidiEvents ---
    let mut events: Vec<MidiEvent> = raw_note_ons
        .iter()
        .map(|(on_tick, ch, note, vel)| {
            let beat = tick_to_beat(*on_tick);

            // Find the first note-off after this note-on.
            let duration_beats = note_off_map
                .get(&(*ch, *note))
                .and_then(|offs| {
                    let idx = offs.partition_point(|&t| t <= *on_tick);
                    offs.get(idx)
                })
                .map(|&off_tick| Beat(tick_to_beat(off_tick).0 - beat.0));

            MidiEvent {
                beat,
                note: *note,
                velocity: *vel,
                channel: *ch,
                duration_beats,
            }
        })
        .collect();

    // Sort events by beat position.
    events.sort_by(|a, b| a.beat.0.partial_cmp(&b.beat.0).unwrap_or(std::cmp::Ordering::Equal));

    // --- Compute total length ---
    let max_tick = raw_note_ons
        .iter()
        .map(|(tick, _, _, _)| *tick)
        .chain(raw_note_offs.iter().map(|(tick, _, _)| *tick))
        .max()
        .unwrap_or(0);

    let length_beats = tick_to_beat(max_tick);

    Ok(MidiSource {
        id,
        path: path.to_path_buf(),
        length_beats,
        embedded_bpm,
        ppq,
        events,
    })
}

/// Load multiple MIDI files, resolving BPM as follows:
///
/// 1. If `override_bpm` is given, all files use it.
/// 2. Otherwise, the first file that has an embedded tempo sets the BPM for all.
/// 3. If no file has an embedded tempo and no override is given, return an error.
///
/// This ensures a consistent BPM across all loaded sources for correct beat alignment.
pub fn load_midi_sources(paths: &[&Path], override_bpm: Option<f64>) -> Result<(Vec<MidiSource>, f64)> {
    if paths.is_empty() {
        bail!("No MIDI files specified");
    }

    // First pass: find effective BPM.
    let effective_bpm = if let Some(bpm) = override_bpm {
        bpm
    } else {
        // Try to find BPM from any of the files.
        let mut found_bpm = None;
        for path in paths {
            let bytes = std::fs::read(path)
                .with_context(|| format!("Cannot read MIDI file: {}", path.display()))?;
            if let Ok(smf) = Smf::parse(&bytes) {
                for track in &smf.tracks {
                    for event in track {
                        if let TrackEventKind::Meta(MetaMessage::Tempo(micros)) = event.kind {
                            found_bpm = Some(60_000_000.0 / micros.as_int() as f64);
                            break;
                        }
                    }
                    if found_bpm.is_some() { break; }
                }
                if found_bpm.is_some() { break; }
            }
        }
        found_bpm.ok_or_else(|| {
            anyhow::anyhow!(
                "No embedded tempo found in any MIDI file and --bpm was not specified"
            )
        })?
    };

    // Second pass: load all files with the resolved BPM.
    let sources: Result<Vec<MidiSource>> = paths
        .iter()
        .map(|path| load_midi(path, Some(effective_bpm)))
        .collect();

    Ok((sources?, effective_bpm))
}
