# Client Integration Guide

How to use `test-fixture.json` (produced by `onset-matcher`) as a golden standard in an application test on any platform — iOS, Android, desktop, or web.

---

## What `test-fixture.json` contains

```json
{
  "schemaVersion": "0.1",
  "name": "my_pattern",
  "bpm": 120.0,
  "timeSignature": [4, 4],
  "events": [
    { "beat": 0.0,  "notes": [36, 42] },
    { "beat": 0.5,  "notes": [42] },
    { "beat": 1.0,  "notes": [36, 38, 42] }
  ]
}
```

Each object in `events` is a **beat cluster**: a group of MIDI note numbers that should all fire at the same beat position. The list is sorted by beat. Note velocity is intentionally omitted — the comparison is instrument-agnostic.

### Field reference

| Field | Type | Description |
|---|---|---|
| `schemaVersion` | string | Always `"0.1"` in this release |
| `name` | string | Human-readable name for this fixture (from `--name` flag) |
| `bpm` | number | Tempo of the arrangement (beats per minute) |
| `timeSignature` | `[num, den]` | e.g. `[4, 4]` for common time |
| `events` | array | Sorted list of beat clusters |
| `events[].beat` | number | Global beat position (0-based, beat 0 = start of arrangement) |
| `events[].notes` | `int[]` | Sorted, deduplicated MIDI note numbers at this beat |

---

## Beat position conventions

Beat positions are **0-based**: beat 0 is the start of the first MIDI layer in the arrangement.

If the arrangement has a beat offset (e.g. `onset-matcher find-arrangement -m pattern.mid=4`), the fixture events will start at beat 4.0, not 0.0. This is intentional — the fixture captures the arrangement as it was specified.

> **For apps that count from beat 1**: subtract 1 from every fixture beat before comparing, or start your own beat counter at 0 for the test.

---

## The comparison protocol

### Step 1 — Run your app and capture note events

Start your music application with the same MIDI files and settings used to generate the fixture. Intercept every MIDI note-on event and record it as a `(beat, noteNumber)` pair:

```
captured_raw = [
  (beat: 0.001, note: 36),
  (beat: 0.002, note: 42),
  (beat: 0.499, note: 42),
  (beat: 1.003, note: 36),
  (beat: 1.003, note: 38),
  (beat: 1.004, note: 42),
  ...
]
```

### Step 2 — Cluster the captured events

Group events that occur within a small beat window (the **clustering tolerance**, e.g. ±0.05 beats) into clusters — one cluster per onset. This mirrors how `onset-matcher` groups simultaneous notes:

```
captured_clusters = [
  { beat: 0.0015, notes: [36, 42] },   # 36 + 42 fired together
  { beat: 0.499,  notes: [42] },
  { beat: 1.003,  notes: [36, 38, 42] },
  ...
]
```

Use the earliest beat in a group as the cluster's representative beat (or the mean — either works for small clusters).

### Step 3 — Compare against the fixture

For each expected cluster in `fixture.events`, find a captured cluster whose beat is within the **matching tolerance** (e.g. ±0.05 beats). The note sets must be identical (same notes, any order).

**Algorithm**:

```
for each expected_cluster in fixture.events:
    match = captured_clusters.find(c => |c.beat - expected_cluster.beat| <= matchTolerance)
    if match is None:
        FAIL: "Missing cluster at beat {expected_cluster.beat} — notes {expected_cluster.notes}"
    if sorted(match.notes) != sorted(expected_cluster.notes):
        FAIL: "Wrong notes at beat {expected_cluster.beat}: got {match.notes}, expected {expected_cluster.notes}"

PASS  (all expected clusters matched)
```

Optionally also fail if there are captured clusters that have no matching expected cluster (the app fired notes that shouldn't be there).

---

## Beat tolerance — choosing the right value

The fixture does not include a tolerance field yet (planned future addition). Choose your matching tolerance based on the precision of your scheduler:

| Scenario | Suggested matching tolerance |
|---|---|
| Sample-accurate DAW/sequencer | ±0.01 beats |
| Real-time MIDI with CoreMIDI scheduling | ±0.05 beats |
| Audio-to-MIDI conversion / transcription | ±0.1 beats |

Use a tight tolerance for a strict test; use a wider tolerance if scheduling jitter is unavoidable.

The clustering tolerance (used in Step 2) should be the same value or slightly tighter than the matching tolerance.

---

## Swift example (iOS/macOS XCTest)

```swift
import XCTest

// --- Models (Codable, matching test-fixture.json schema) ---

struct BeatFixture: Decodable {
    let schemaVersion: String
    let name: String
    let bpm: Double
    let timeSignature: [Int]
    let events: [BeatCluster]
}

struct BeatCluster: Decodable {
    let beat: Double
    let notes: [Int]
}

// --- Simple comparison helper ---

func assertBeatFixture(
    _ fixture: BeatFixture,
    matches captured: [BeatCluster],
    tolerance: Double = 0.05,
    file: StaticString = #file,
    line: UInt = #line
) {
    for expected in fixture.events {
        guard let match = captured.first(where: { abs($0.beat - expected.beat) <= tolerance }) else {
            XCTFail(
                "Missing cluster at beat \(expected.beat): expected notes \(expected.notes)",
                file: file, line: line
            )
            continue
        }
        XCTAssertEqual(
            match.notes.sorted(), expected.notes.sorted(),
            "Wrong notes at beat \(expected.beat): got \(match.notes.sorted()) expected \(expected.notes.sorted())",
            file: file, line: line
        )
    }
}

// --- In your test ---

class DrumMachineTests: XCTestCase {

    func testKickLoopMatchesFixture() throws {
        // 1. Load fixture
        let fixtureURL = Bundle.module.url(forResource: "kick_loop", withExtension: "json")!
        let fixture = try JSONDecoder().decode(BeatFixture.self, from: Data(contentsOf: fixtureURL))

        // 2. Run your engine and capture events
        let engine = MyDrumEngine()
        var rawEvents: [(beat: Double, note: Int)] = []

        engine.onNoteOn = { beat, note in
            rawEvents.append((beat: beat, note: Int(note)))
        }
        engine.play()
        // Wait for the pattern to complete (fixture.bpm tells you the tempo)
        let patternDurationSeconds = (fixture.events.last?.beat ?? 0) * 60.0 / fixture.bpm + 1.0
        Thread.sleep(forTimeInterval: patternDurationSeconds)
        engine.stop()

        // 3. Cluster raw events
        let clusteringTolerance = 0.05
        var clusters: [BeatCluster] = []
        for raw in rawEvents.sorted(by: { $0.beat < $1.beat }) {
            if let last = clusters.last, abs(last.beat - raw.beat) <= clusteringTolerance {
                // Merge into existing cluster
                let merged = BeatCluster(beat: last.beat, notes: (last.notes + [raw.note]).sorted())
                clusters[clusters.count - 1] = merged
            } else {
                clusters.append(BeatCluster(beat: raw.beat, notes: [raw.note]))
            }
        }

        // 4. Compare
        assertBeatFixture(fixture, matches: clusters, tolerance: 0.05)
    }
}
```

---

## What the comparison intentionally ignores

| Ignored | Why |
|---|---|
| Note velocity | Instrument-dependent; omitted from fixture deliberately |
| Note duration | Only note-on events are compared |
| MIDI channel | Channel assignment is irrelevant to musical equivalence |
| Audio waveform | The fixture never references audio |
| Instrument sound | The test is about *which notes at which beats*, not timbre |

---

## What makes a test fail

- A note cluster is **missing entirely** — your app skipped a beat
- A note cluster fires at the **wrong beat** — outside the matching tolerance
- A note cluster fires the **wrong notes** — different drum sounds or pitch

---

## Further reading

- [`plans/beat-fixture-kit.md`](../plans/beat-fixture-kit.md) — design plan for `BeatFixtureKit`, a Swift Package that provides `BeatEventRecorder`, `BeatFixtureMatcher`, and `BeatFixtureMatchResult` as reusable, drop-in components for XCTest
- [`README.md`](../README.md) — full `onset-matcher` documentation including how to generate `test-fixture.json`
