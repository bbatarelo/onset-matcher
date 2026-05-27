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

## Project Status

Feature 1 is complete and the project is runnable. Domain types grow incrementally alongside features.

```
onset-matcher show-onsets --bpm=120 reference.wav
```

The planned feature milestones are:

**Feature 1 — `show-onsets`** ✅: Load audio, detect onsets, map to beat grid, render as a text grid in the terminal. Requires user to supply `--bpm`.

**Feature 2 — MIDI display**: Load MIDI files, display their event patterns in the same console grid alongside the audio onsets.

**Feature 3 — Arrangement scoring**: Given user-specified MIDI layer placements, score them against the audio onset curve and show how well the MIDI explains the audio.

**Feature 4 — Arrangement search**: Automatically search for the arrangement of MIDI layers that best explains the audio.

**Feature 5 — Canonical export**: Compute diagnostics and produce both output files (`canonical.json` and `test-fixture.json`).

---

## Maintenance Note

This README is the **living context document** for the project. It should be updated whenever:
- The problem statement or use case is refined
- Core abstractions change
- New design decisions are made
- The implementation status changes
