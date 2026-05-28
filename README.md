# onset-matcher

A **MIDI-guided reference-audio alignment and arrangement inference tool** written in Rust.

---

## What is this?

`onset-matcher` solves a specific problem in **music software testing**: it is hard to automate unit tests for music applications when the expected output is audio, because raw waveform comparison is fragile — the same musical pattern played by different drum kits or synthesizers produces completely different waveforms.

This tool bridges the gap. Instead of comparing waveforms, it compares **MIDI event sequences at beat positions** — a representation that is instrument-agnostic.

### The workflow

1. A developer records a **reference audio file** of rhythmic content (a drum machine, a drummer, etc.) playing a known pattern at a known BPM.
2. The developer provides the **MIDI files** that are supposed to represent that pattern (e.g. the patterns loaded into the drum machine).
3. `onset-matcher` analyses the audio, aligns the MIDI files to it, and produces a **canonical JSON file** — a golden standard describing exactly which MIDI files should play, in what order, with what overlaps, at what beat positions.
4. When testing a music application, developers generate a canonical description of their module's MIDI output and compare it against the golden standard. The test passes or fails based on musical equivalence, not waveform similarity.

---

## The Core Idea

> **The audio is evidence. The MIDI files are candidate symbolic material. The inferred arrangement is the result.**

The tool does not export audio information. It exports a description of **how to play the MIDI files** so that the result is musically equivalent to the reference audio.

---

## Why Not Just Compare Waveforms?

Waveform comparison:
- Fails if the instrument sound changes (different drum kit, different reverb)
- Fails if there is any clock jitter or timing drift
- Produces meaningless diffs ("sample 44103 differs by 0.002")
- Cannot be used as a human-readable specification

Beat-position MIDI comparison:
- Is instrument-agnostic
- Tolerates clock jitter (the tool corrects for it during analysis)
- Produces meaningful diffs ("kick drum at beat 3.0 is missing")
- Is human-readable and can serve as a specification document

---

## Clock Jitter — The Noise Problem

When a drum machine plays at 120 BPM and the audio is recorded on a computer, the drum machine's internal clock and the computer's clock are not identical. Onsets in the recorded audio will land slightly before or after the ideal beat grid.

`onset-matcher` treats this jitter as **measurement noise to reason through**, not as meaningful data to encode. The canonical output always uses a clean, jitter-free beat grid. The tool's job is to find the arrangement that best explains the audio despite the jitter.

---

## What "Arrangement" Means

A reference audio recording may have been produced by multiple MIDI patterns playing in sequence or overlapping. For example:

```
beat:   0        8        16       24       32
A:      [--------]
B:               [--------]
C:                        [------]
D:                           [------]
```

Here pattern A plays first, then B, then C and D overlap. `onset-matcher` infers this structure from the audio and expresses it in the canonical output as a set of **layers**, each with a start beat and end beat.

---

## How It Works (Pipeline)

```
1.  Load reference audio          →  decode WAV/FLAC/MP3 to PCM samples
2.  Compute onset-strength curve  →  frame-based transient energy per time step
3.  Detect discrete onset peaks   →  list of observed onsets in seconds
4.  Load MIDI source files        →  list of MIDI patterns with beat-based events
5.  Define search space           →  which MIDIs, possible start beats, overlap rules
6.  Generate candidate arrangements
7.  For each candidate:
      a. Render to expected onset template
      b. Score against audio onset curve
8.  Choose best arrangement
9.  Map observed onsets to beat grid   →  corrects for clock jitter
10. Compute diagnostics                →  coverage, confidence, timing error
11. Export canonical JSON              →  the golden test standard
12. (Optional) Export diagnostic report →  visual verification for humans
```

The core mathematical operation (step 7b) is **template matching**: the tool generates a frame-based curve of expected onset positions from each candidate arrangement and slides it against the audio onset curve, finding the offset that maximises alignment. This is essentially a 1D cross-correlation search.

---

## Two Coordinate Systems

One of the most important design rules in this codebase: **audio time and musical time are always kept separate**.

| Coordinate | Unit | Used for |
|---|---|---|
| **Audio time** | seconds / sample frames | Raw audio, onset peaks, recording position |
| **Musical time** | beats / bars / ticks | MIDI events, arrangement layers, canonical export |

These are never mixed. The bridge between them is `TimeMap`, which holds BPM, time signature, and the position of beat 0 in the audio file.

Rust newtype wrappers (`Beat(f64)` and `Seconds(f64)`) make it a compile-time error to accidentally use one where the other is expected.

---

## Onsets Exist in Two Forms

Onsets from the reference audio appear in two representations:

**Frame-based onset curve** (`AudioOnsetCurve`): a continuous-ish signal of onset strength over time, used by the matcher to score candidate arrangements.

**Discrete onset peaks** (`ObservedOnset`): individual transient candidates extracted from the curve, used for reporting and for mapping to the beat grid.

After the best arrangement is found, the discrete onsets are also expressed as **grid onsets** (`GridOnset`): mapped to beat space, with their quantization error recorded.

---

## MIDI Sources vs. Arrangement Layers

A **MIDI source** (`MidiSource`) is raw material: a parsed MIDI file with beat-based note events. It does not have a position in time.

An **arrangement layer** (`ArrangementLayer`) is a placed instance of a MIDI source on the global beat timeline: it says "play this MIDI source starting at beat 12, ending at beat 20." Multiple layers can be active simultaneously — overlap is a first-class concept.

---

## The Canonical Output

The canonical JSON file describes:
- Tempo and time signature
- Which MIDI source files are involved
- Where each layer starts and ends (in beats)
- Every expected note event (beat position, note number, velocity)
- Onset clusters (groups of simultaneous notes) with confidence scores
- Provenance metadata (which audio file, match score, tool version)

Example:

```json
{
  "schemaVersion": "0.1",
  "name": "my_test_pattern",
  "musicalContext": {
    "bpm": 120.0,
    "timeSignature": [4, 4],
    "ppq": 960
  },
  "sources": [
    { "id": "midi_a", "path": "pattern_a.mid", "lengthBeats": 16.0 },
    { "id": "midi_b", "path": "fill_b.mid",    "lengthBeats": 4.0 }
  ],
  "layers": [
    { "id": "layer_1", "sourceId": "midi_a", "startBeat": 0.0,  "endBeat": 16.0, "sourceStartBeat": 0.0 },
    { "id": "layer_2", "sourceId": "midi_b", "startBeat": 14.0, "endBeat": 18.0, "sourceStartBeat": 0.0 }
  ],
  "events": [
    { "beat": 0.0,  "sourceId": "midi_a", "layerId": "layer_1", "sourceBeat": 0.0,  "note": 36, "velocity": 110 },
    { "beat": 14.0, "sourceId": "midi_b", "layerId": "layer_2", "sourceBeat": 0.0,  "note": 49, "velocity": 120 }
  ],
  "onsetClusters": [
    { "beat": 14.0, "eventIndices": [1], "expectedStrength": 0.95, "confidence": 0.92 }
  ],
  "provenance": {
    "generatedBy": "onset-matcher 0.1",
    "sourceAudio": "reference.wav",
    "matchScore": 0.93,
    "reviewStatus": "auto"
  }
}
```

This file does not reference the original audio. A test can use it without the audio file.

---

## Using the Canonical File in Tests

A music application under test can:

1. Load the same MIDI source files
2. Play them back according to the canonical layer arrangement
3. Capture the MIDI events emitted
4. Serialize those events to a canonical description
5. Compare against the golden canonical JSON

The test succeeds if the events match at the correct beat positions (within a configurable tolerance). The instrument sound, audio quality, and clock jitter are all irrelevant to the comparison.

---

## Domain Vocabulary

| Term | Meaning |
|---|---|
| `AudioOnsetCurve` | Frame-based onset strength extracted from reference audio |
| `ObservedOnset` | Discrete transient candidate from reference audio (in seconds) |
| `GridOnset` | Observed onset mapped to beat space after alignment |
| `MidiSource` | Parsed MIDI file as beat-based symbolic material (no position yet) |
| `ArrangementLayer` | One placed instance of a MidiSource on the global beat timeline |
| `Arrangement` | Collection of possibly overlapping ArrangementLayers + TimeMap |
| `ExpectedEvent` | Individual MIDI note rendered onto global beat timeline |
| `ExpectedOnsetCluster` | Group of simultaneous or near-simultaneous ExpectedEvents |
| `MidiOnsetTemplate` | Frame-based curve generated from an Arrangement (parallels AudioOnsetCurve) |
| `AlignmentResult` | Analyzer output: best arrangement, alternatives, diagnostics |
| `CanonicalPlaybackReference` | The golden standard JSON output |
| `AlignmentReport` | Visual/diagnostic output for human review |

---

## Architecture

See [`plans/architecture.md`](plans/architecture.md) for the detailed technical architecture: module structure, all Rust type definitions, dependency choices, implementation phases, and the scoring algorithm.

---

## Two Output Formats

`onset-matcher` produces two output files from the same analysis run:

### 1. `canonical.json` — Full Rich Format

The complete authoring record: all layers, source IDs, onset clusters, confidence scores, provenance metadata. This is the archive format and the source of truth for the analysis. It can be used to re-run or audit the alignment.

### 2. `test-fixture.json` — Flattened Test Format

A minimal, app-friendly format for use in unit tests in any platform or language. It contains only what an application under test needs to verify musical equivalence:

```json
{
  "schemaVersion": "0.1",
  "name": "my_test_pattern",
  "bpm": 120.0,
  "timeSignature": [4, 4],
  "events": [
    { "beat": 0.0,  "notes": [36, 42] },
    { "beat": 0.5,  "notes": [42] },
    { "beat": 1.0,  "notes": [36, 38, 42] },
    { "beat": 1.5,  "notes": [42] }
  ]
}
```

This is generated automatically by `onset-matcher` by flattening the full canonical event list: all events are grouped by beat cluster, note velocity is omitted, and layer/source IDs are dropped. The result is trivially consumable from any language.

An iOS app test, for example, only needs to:
1. Capture `(beat, [notes])` tuples from its sequencer output
2. Load `test-fixture.json`
3. Compare the two lists

No knowledge of `onset-matcher`'s internal model is needed on the consumer side.

#### Beat tolerance (future)

The fixture currently has no tolerance field — comparison is exact. Beat tolerance (e.g. "events within ±0.05 beats are considered matching") is a planned future addition. Open questions to resolve when adding it:
- Should tolerance be fixed (e.g. always ±0.05 beats)?
- Should it be derived from the match quality score in the canonical JSON (lower confidence = wider tolerance)?
- Should it be user-specified per test?
- Should velocity optionally be included with a tolerance band?

---

## Usage Examples

All subcommands are built and runnable. Replace file paths with your own.

### `show-onsets` — visualise audio onsets on a beat grid

```sh
# Minimum: supply audio file and BPM
onset-matcher show-onsets --audio reference.wav --bpm 120

# With 16th-note grid, onset strength column, and sparkline
onset-matcher show-onsets --audio reference.wav --bpm 120 \
  --subdivision 0.25 --show-strength --show-curve

# Trim the grid so beat 0 starts at the first detected onset
onset-matcher show-onsets --audio reference.wav --bpm 120 --trim-audio

# Disable auto beat-zero refinement (use raw BPM alignment)
onset-matcher show-onsets --audio reference.wav --bpm 120 --no-refine-beat-zero

# Raise the detection threshold (fewer, stronger onsets)
onset-matcher show-onsets --audio reference.wav --bpm 120 --threshold 0.3
```

### `show-midi` — visualise MIDI note events on a beat grid

```sh
# Single file — BPM read from the MIDI tempo track
onset-matcher show-midi -m pattern.mid

# Explicit BPM override
onset-matcher show-midi --bpm 120 -m pattern.mid

# Two files, second starts at beat 8
onset-matcher show-midi --bpm 120 -m intro.mid=0 -m verse.mid=8

# Same file repeated at different offsets
onset-matcher show-midi --bpm 120 -m loop.mid=0 -m loop.mid=16
```

### `compare` — audio onsets and MIDI notes on the same grid

```sh
# Minimum: audio + one MIDI file
onset-matcher compare --audio reference.wav -m pattern.mid

# Multiple MIDI files with explicit beat offsets
onset-matcher compare --audio reference.wav --bpm 120 \
  -m intro.mid=0 -m verse.mid=8 -m chorus.mid=24

# Show onset strength and sparkline
onset-matcher compare --audio reference.wav -m pattern.mid \
  --show-strength --show-curve

# Trim audio so bar 1 starts at first onset
onset-matcher compare --audio reference.wav -m pattern.mid --trim-audio
```

### `score-arrangement` — score how well explicit MIDI placements explain the audio

```sh
# Minimum: audio + at least one MIDI file with beat offset
onset-matcher score-arrangement --audio reference.wav \
  -m pattern.mid=0

# Multiple layers (same file repeated, or different patterns)
onset-matcher score-arrangement --audio reference.wav --bpm 120 \
  -m intro.mid=0 -m verse.mid=8 -m verse.mid=16 -m chorus.mid=24

# Relax the beat-matching tolerance (default 0.5 beat)
onset-matcher score-arrangement --audio reference.wav \
  -m pattern.mid=0 --match-tolerance 1.0

# Fine-tune threshold + show visual grid
onset-matcher score-arrangement --audio reference.wav \
  -m pattern.mid=0 --threshold 0.2 --show-strength --trim-audio
```

### `find-arrangement` — automatically find the best beat offsets for MIDI files

```sh
# Single source: try offset 0 only (equivalent to score-arrangement with offset=0)
onset-matcher find-arrangement --audio reference.wav -m pattern.mid

# Search offsets 0–32 in steps of 4 beats (every bar in 4/4)
onset-matcher find-arrangement --audio reference.wav \
  -m intro.mid --search-min 0 --search-max 32 --search-step 4

# Two sources: search all combinations of 0..16 in 1-beat steps
onset-matcher find-arrangement --audio reference.wav --bpm 120 \
  -m verse.mid -m fill.mid \
  --search-min 0 --search-max 16 --search-step 1

# Widen the Gaussian template window (more tolerant of timing variation)
onset-matcher find-arrangement --audio reference.wav \
  -m pattern.mid --search-max 8 --event-window 0.05

# Show both onset curves (audio + expected) as sparklines
onset-matcher find-arrangement --audio reference.wav \
  -m pattern.mid --search-max 8 --show-curve --show-expected-curve

# Trim audio beat-zero before searching
onset-matcher find-arrangement --audio reference.wav \
  -m pattern.mid --search-max 8 --trim-audio
```

---

### Common flags (all audio subcommands)

| Flag | Default | Meaning |
|---|---|---|
| `--audio <FILE>` | — | Reference audio file (WAV, FLAC, MP3, OGG, …) |
| `--bpm <N>` | from MIDI | Tempo; required for `show-onsets`, optional elsewhere |
| `--time-sig-num <N>` | `4` | Beats per bar |
| `--time-sig-den <N>` | `4` | Beat note value |
| `--subdivision <N>` | `0.25` | Grid resolution (0.25 = 16th, 0.5 = 8th, 1.0 = quarter) |
| `--bars-per-row <N>` | `4` | Terminal layout width |
| `--threshold <N>` | `0.15` | Onset detection sensitivity (lower = more onsets) |
| `--refine-beat-zero` | `true` | Snap beat 0 to nearest integer beat from first strong onset |
| `--trim-audio` | off | Set beat 0 = first onset ≥ threshold (exclusive with `--refine-beat-zero`) |
| `--show-strength` | off | Print onset strength value next to each onset marker |
| `--show-curve` | off | Print ASCII sparkline of the onset-strength curve |

### `--midi-file` / `-m` flag

```sh
-m path/to/file.mid          # beat offset defaults to 0
-m path/to/file.mid=8        # beat offset = 8
-m path/to/file.mid=8.5      # fractional beat offsets are fine
```

The flag is repeatable. The same file may appear multiple times at different offsets.

---

## Project Status

Features 1–3 are complete and the project is runnable. Domain types grow incrementally alongside features.

The planned feature milestones are:

**Feature 1 — `show-onsets`** ✅
Load audio, detect onsets, map to beat grid, render as a text grid in the terminal.

**Feature 2 — `show-midi` / `compare`** ✅
Load MIDI files, display their note-on events in the same console grid format (`show-midi`); display audio onsets and MIDI notes together on one shared grid (`compare`). BPM is read from the MIDI tempo track or supplied via `--bpm`.

**Feature 3 — Arrangement scoring** ✅
Given user-specified MIDI layer placements (via `-m file=beat_offset`), score them against the audio onset curve and show per-cluster match diagnostics.

**Feature 4 — Arrangement search** ✅
Automatically search for the beat offsets for each MIDI file that maximise overlap with the audio onset curve (`find-arrangement`). Uses Gaussian-bump cross-correlation: for each candidate arrangement, a continuous expected-onset curve is generated and scored against the audio. Search space is controlled by `--search-min`, `--search-max`, `--search-step` (per source). Displays the best arrangement on the console grid with properly calibrated audio alignment.

**Feature 5 — Canonical export**: Compute diagnostics and produce both output files (`canonical.json` and `test-fixture.json`).

---

## Maintenance Note

This README is the **living context document** for the project. It should be updated whenever:
- The problem statement or use case is refined
- Core abstractions change
- New design decisions are made
- The implementation status changes
