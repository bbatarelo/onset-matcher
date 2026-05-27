# onset-matcher — Architecture Plan

## Purpose

`onset-matcher` is a **MIDI-guided reference-audio alignment and arrangement inference tool**.

The concrete use case is **music software testing**: a developer records reference audio of rhythmic content (e.g. a drum machine playing a pattern), provides the MIDI files that were supposed to produce that pattern, and the tool produces a **canonical golden-standard JSON** describing how those MIDI files should be arranged and played back to produce a musically equivalent result. This canonical file can then be used in automated tests to verify that a music application emits the correct MIDI events at the correct beats — without needing raw waveform comparison.

Clock jitter between the recording device and the computer is treated as measurement noise to reason through, **not** something encoded in the output. The canonical export is always clean and jitter-free.

---

## The Problem in One Paragraph

> Given a reference audio recording of rhythmic content and one or more MIDI files (with known or provided BPM), find which MIDI files were being played, in what order and with what overlaps, so that when those MIDI files are played back perfectly at the stated BPM the result is musically equivalent to the reference audio. Export this as a canonical JSON playback specification suitable for use as a golden test fixture.

---

## Pipeline

```
1.  Load reference audio  →  decode WAV/FLAC/etc. to PCM samples
2.  Compute onset-strength curve  →  frame-based transient energy (AudioOnsetCurve)
3.  Detect discrete onset peaks  →  list of ObservedOnsets in seconds
4.  Load MIDI source files  →  list of MidiSources with beat-based events
5.  Define search space  →  ArrangementSearchSpec (which MIDIs, start beat ranges, overlap rules)
6.  Generate candidate arrangements  →  list of candidate Arrangements
7.  For each candidate:
      a. Render arrangement  →  ExpectedOnsetClusters + MidiOnsetTemplate
      b. Score template vs. audio onset curve
8.  Choose best arrangement
9.  Map observed onsets onto beat grid  →  GridOnsets (using TimeMap + best arrangement)
10. Compute diagnostics  →  coverage, timing error, unexplained onsets, confidence
11. Export canonical JSON  →  CanonicalPlaybackReference
12. (Optional) Export diagnostic report  →  SVG/HTML for human review
```

---

## Two Coordinate Systems — the Most Important Design Rule

Everything in the system works in one of two time coordinates:

| Coordinate | Unit | Used for |
|---|---|---|
| **Audio time** | seconds / sample frames | Raw audio, onset peaks, recording position |
| **Musical time** | beats / bars / ticks | MIDI events, arrangement layers, canonical export |

The bridge between them is [`TimeMap`](../src/domain/time.rs):

```rust
pub struct TimeMap {
    pub bpm: f64,
    pub time_signature: TimeSignature,
    pub beat_zero_seconds: Seconds,  // where beat 0 sits in the audio file
}
```

Conversion (constant tempo):
```
seconds = beat_zero_seconds + beat * 60.0 / bpm
beat    = (seconds - beat_zero_seconds) / (60.0 / bpm)
```

Newtype wrappers enforce that you never accidentally mix the two:
```rust
pub struct Beat(pub f64);
pub struct Seconds(pub f64);
pub struct FrameIndex(pub usize);
```

---

## Domain Type Map

### Time primitives — `src/domain/time.rs`

- `Beat(f64)` — position in musical beat space
- `Seconds(f64)` — position in audio time
- `FrameIndex(usize)` — sample frame index
- `TimeSignature { numerator: u8, denominator: u8 }`
- `TimeMap { bpm, time_signature, beat_zero_seconds }` — the coordinate bridge

---

### Audio analysis — `src/domain/audio.rs`

```
AudioOnsetCurve         frame-based onset strength (input to matcher)
ObservedOnset           discrete peak extracted from the curve (in seconds)
GridOnset               same peak after mapping to beat space
AudioAnalysis           container: source path + curve + observed onsets
AlignedAudioAnalysis    container: time_map + grid onsets (post-alignment)
```

**`AudioOnsetCurve`** is what MIDI templates slide over — a `Vec<f32>` of onset strength per frame.

**`ObservedOnset`** is a discrete transient candidate:
```rust
pub struct ObservedOnset {
    pub time_seconds: Seconds,
    pub strength: f32,
    pub width_seconds: Option<f64>,
    pub confidence: Option<f64>,
}
```

**`GridOnset`** is the same onset mapped to beat space after alignment:
```rust
pub struct GridOnset {
    pub time_seconds: Seconds,
    pub beat: Beat,
    pub bar: Option<u32>,
    pub beat_in_bar: Option<f64>,
    pub strength: f32,
    pub quantization_error_beats: f64,
}
```

---

### MIDI sources — `src/domain/midi.rs`

```
SourceId                newtype string ID
MidiSource              parsed MIDI file: path, length_beats, ppq, events
MidiEvent               one note at a beat offset with velocity/channel
```

A `MidiSource` is **available symbolic material only** — it has no position in the arrangement yet.

---

### Arrangement — `src/domain/arrangement.rs`

```
LayerId                 newtype string ID
ArrangementLayer        one placed MidiSource on the global beat timeline
Arrangement             TimeMap + Vec<ArrangementLayer>
```

```rust
pub struct ArrangementLayer {
    pub layer_id: LayerId,
    pub source_id: SourceId,
    pub start_beat: Beat,   // global beat where this layer begins
    pub end_beat: Beat,     // global beat where this layer ends
    pub source_start_beat: Beat,  // which beat inside the MIDI to start from
    pub gain: Option<f32>,
    pub role: Option<String>,
}
```

Overlap between layers is natural — multiple layers can be active simultaneously.

---

### Expected events — `src/domain/expected.rs`

```
ExpectedEvent           one MIDI event rendered onto global beat timeline
ExpectedOnsetCluster    group of simultaneous/near-simultaneous ExpectedEvents
MidiOnsetTemplate       frame-based curve generated from an Arrangement (like AudioOnsetCurve but symbolic)
```

The matcher compares `AudioOnsetCurve` against `MidiOnsetTemplate` frame-by-frame.

---

### Alignment result & diagnostics — `src/domain/result.rs`

```
AlignmentResult         top-level analyzer output
ArrangementMatch        one candidate arrangement with its score and matched clusters
MatchedOnsetCluster     one expected cluster paired with its observed onset (if any)
AlignmentDiagnostics    summary statistics: coverage, timing error, unexplained onsets
AlignmentWarning        enum of named warning conditions
```

---

### Canonical export — `src/domain/canonical.rs`

```
CanonicalPlaybackReference    the golden JSON output
CanonicalMusicalContext       bpm, time signature, ppq
CanonicalLayer                layer placement (start/end beats)
CanonicalEvent                one note at a global beat
CanonicalOnsetCluster         cluster of events at a beat with confidence
ReferenceProvenance           metadata: tool version, source audio path, match score
```

The canonical file describes **how to play the MIDI files**, not the audio. The audio is discarded after inference.

---

### Scenario / input spec — `src/domain/scenario.rs`

```
AlignmentScenario         top-level input: audio path, MIDI sources, context, search spec
MusicalContext            bpm (known or to be estimated), time signature, beat_zero search range
ArrangementSearchSpec     which layers to search and how
LayerSearchSpec           one layer's search parameters
BeatSearchSpec            Fixed / Range / OnGrid variants
```

---

## Module Structure

```
src/
  main.rs                     CLI entry point
  domain/
    mod.rs
    time.rs                   TimeMap, Beat, Seconds, FrameIndex, TimeSignature
    audio.rs                  AudioOnsetCurve, ObservedOnset, GridOnset, AudioAnalysis
    midi.rs                   MidiSource, MidiEvent, SourceId
    arrangement.rs            ArrangementLayer, Arrangement, LayerId
    expected.rs               ExpectedEvent, ExpectedOnsetCluster, MidiOnsetTemplate
    result.rs                 AlignmentResult, ArrangementMatch, MatchedOnsetCluster, AlignmentDiagnostics
    canonical.rs              CanonicalPlaybackReference and sub-types
    scenario.rs               AlignmentScenario, ArrangementSearchSpec, BeatSearchSpec
  pipeline/
    mod.rs
    audio_analysis.rs         load audio → AudioOnsetCurve + ObservedOnsets
    onset_detection.rs        peak picking from onset curve
    midi_loader.rs            parse MIDI files → MidiSource
    arrangement_renderer.rs   Arrangement → ExpectedOnsetClusters + MidiOnsetTemplate
    scorer.rs                 AudioOnsetCurve vs MidiOnsetTemplate → score
    search.rs                 ArrangementSearchSpec → ranked candidates
    beat_mapper.rs            ObservedOnsets + TimeMap → GridOnsets
    diagnostics.rs            build AlignmentDiagnostics from match result
    canonical_builder.rs      AlignmentResult → CanonicalPlaybackReference
    scenario_loader.rs        JSON → AlignmentScenario
```

---

## Dependencies (Cargo.toml)

| Crate | Purpose |
|---|---|
| `symphonia` | Audio file decoding (WAV, FLAC, MP3, OGG) |
| `midly` | MIDI file parsing |
| `serde` + `serde_json` | Serialization of scenario input and canonical output |
| `clap` | CLI argument parsing |
| `rustfft` or `realfft` | FFT for spectral onset detection (optional, depending on algorithm) |

---

## Scoring Algorithm (template matching)

The core mathematical problem: given `AudioOnsetCurve` A and `MidiOnsetTemplate` T, find the time offset (beat_zero) that maximises alignment.

For a **fixed BPM** scenario this is a 1D search:
1. Render T at a given `beat_zero` candidate
2. Compute cross-correlation or cosine similarity between A and T
3. The offset with the highest score is the best `beat_zero`

For the **arrangement search**, this extends to:
1. For each candidate arrangement (different layer start beats)
2. Render T
3. Score vs A
4. Return ranked list

This can be implemented as a sliding window cross-correlation, which is O(N·K) where N is audio frames and K is the number of candidate offsets.

---

## Two Export Formats

`onset-matcher` produces two output files from one analysis run.

### Format 1: `canonical.json` — Full Rich Format

The complete archival record. Consumed by `onset-matcher` itself (for re-analysis, auditing). Contains all layers, sources, onset clusters, confidence, provenance. Described below.

### Format 2: `test-fixture.json` — Flattened Test Format

Minimal consumer format for unit tests in any language/platform. Generated by flattening the canonical event list: group all events by beat cluster, drop velocity, drop source/layer IDs.

```rust
pub struct TestFixture {
    pub schema_version: String,
    pub name: String,
    pub bpm: f64,
    pub time_signature: TimeSignature,
    pub events: Vec<TestFixtureEvent>,
}

pub struct TestFixtureEvent {
    pub beat: Beat,
    pub notes: Vec<u8>,
    // velocity intentionally omitted for simplicity
    // beat_tolerance: future addition (see below)
}
```

#### Beat tolerance — future work

The `test-fixture.json` currently has no tolerance field; comparison is exact on beat position. Beat tolerance is a planned future feature. Open design questions to resolve:
- Fixed tolerance (e.g. always ±0.05 beats)?
- Derived from canonical match confidence score (lower confidence → wider tolerance)?
- User-specified per test scenario?
- Optionally include velocity with a tolerance band for stricter tests?

---

## Canonical JSON Shape

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
    { "id": "midi_a", "path": "a.mid", "lengthBeats": 16.0 },
    { "id": "midi_b", "path": "b.mid", "lengthBeats": 8.0 }
  ],
  "layers": [
    { "id": "layer_1", "sourceId": "midi_a", "startBeat": 0.0, "endBeat": 16.0, "sourceStartBeat": 0.0 },
    { "id": "layer_2", "sourceId": "midi_b", "startBeat": 12.0, "endBeat": 20.0, "sourceStartBeat": 0.0 }
  ],
  "events": [
    { "beat": 0.0,  "sourceId": "midi_a", "layerId": "layer_1", "sourceBeat": 0.0,  "note": 36, "velocity": 110 },
    { "beat": 12.0, "sourceId": "midi_b", "layerId": "layer_2", "sourceBeat": 0.0,  "note": 49, "velocity": 120 }
  ],
  "onsetClusters": [
    { "beat": 12.0, "eventIndices": [1], "expectedStrength": 0.95, "confidence": 0.92 }
  ],
  "provenance": {
    "generatedBy": "onset-matcher 0.1",
    "sourceAudio": "reference.wav",
    "matchScore": 0.93,
    "reviewStatus": "auto"
  }
}
```

---

## Implementation Strategy

Domain types grow incrementally alongside features — only the types needed for a given feature are introduced when that feature is implemented. This keeps the codebase grounded in working, runnable code at every step.

---

## Feature 1 — Audio Onset Visualisation (`--show-onsets`)

**Goal**: Given an audio file and a BPM, detect onsets and render them as a text grid in the terminal so a developer can visually verify onset detection quality.

**CLI**:
```
onset-matcher --bpm=120 --show-onsets reference.wav
```

**Domain types introduced** (all others deferred):
- `Beat`, `Seconds`, `FrameIndex` — newtype wrappers
- `TimeSignature`, `TimeMap` — coordinate bridge
- `AudioOnsetCurve` — frame-based onset strength
- `ObservedOnset` — discrete onset peak in seconds
- `GridOnset` — onset mapped to beat/bar space
- `AudioAnalysis` — container

**Pipeline for Feature 1**:
```
1. Parse CLI: audio path, --bpm, optional --time-sig (default 4/4)
2. Load audio via symphonia → PCM samples
3. Compute onset-strength curve → AudioOnsetCurve
4. Detect peaks → Vec<ObservedOnset>
5. Construct TimeMap from --bpm (beat_zero_seconds = 0.0 initially)
6. Map onsets to beat grid → Vec<GridOnset>
7. Render to console
```

**Console renderer** — text grid layout:

```
BPM: 120  |  Time signature: 4/4  |  Duration: 8 bars

Bar  Beat: 1    +    2    +    3    +    4    +
  1        •         •              •
  2        •         •         •         •
  3        •              •         •
  4   •         •         •         •
...
```

Each cell represents a 1/8th-note subdivision (configurable). `•` marks a detected onset. Strength can be indicated by character weight (`·` weak, `•` medium, `●` strong).

**New modules introduced**:
- `src/domain/time.rs` — time primitives + `TimeMap`
- `src/domain/audio.rs` — audio analysis types
- `src/pipeline/audio_analysis.rs` — load audio + compute onset curve
- `src/pipeline/onset_detection.rs` — peak picking
- `src/pipeline/beat_mapper.rs` — `TimeMap` construction + onset→`GridOnset` mapping
- `src/render/console.rs` — text grid renderer

**Dependencies introduced**:
- `symphonia` — audio file decoding (WAV, FLAC, MP3, OGG)
- `clap` — CLI argument parsing
- `serde` + `serde_json` — added now, used in later features

---

## Feature 2 — MIDI Loader and Arrangement Display

**Goal**: Load one or more MIDI files, parse them to beat-based events, and display their event patterns in the same console grid format alongside the audio onsets.

**New domain types**: `SourceId`, `MidiSource`, `MidiEvent`, `LayerId`, `ArrangementLayer`, `Arrangement`

**New dependency**: `midly`

---

## Feature 3 — Arrangement Scoring

**Goal**: Given a proposed arrangement (user-specified layer placements), render expected onset clusters and score them against the audio onset curve.

**New domain types**: `ExpectedEvent`, `ExpectedOnsetCluster`, `MidiOnsetTemplate`

---

## Feature 4 — Arrangement Search

**Goal**: Given a search specification, automatically find the arrangement of MIDI layers that best explains the audio onsets.

**New domain types**: `AlignmentScenario`, `MusicalContext`, `ArrangementSearchSpec`, `LayerSearchSpec`, `BeatSearchSpec`

---

## Feature 5 — Diagnostics and Canonical Export

**Goal**: Compute quality metrics, build the canonical JSON and flattened test fixture, wire the full pipeline.

**New domain types**: `AlignmentResult`, `ArrangementMatch`, `MatchedOnsetCluster`, `AlignmentDiagnostics`, `AlignmentWarning`, `CanonicalPlaybackReference` and sub-types, `TestFixture`, `TestFixtureEvent`

**Outputs**: `canonical.json` (full rich format) + `test-fixture.json` (flattened, instrument-agnostic)

---

## Key Design Principles

1. **Audio time ≠ musical time** — always use `Beat` and `Seconds` newtypes, never raw `f64` for both
2. **Onsets exist in two forms** — continuous curve for matching, discrete peaks for reporting
3. **MIDI sources are not arrangements** — a `MidiSource` is material; an `ArrangementLayer` is a placement
4. **Clusters, not raw events, are matched against audio** — multiple simultaneous MIDI notes produce one audio transient
5. **The audio is evidence; the canonical export is the result** — the audio file is not referenced in the output
6. **Clock jitter is noise** — the canonical output is always a clean, jitter-free beat grid
