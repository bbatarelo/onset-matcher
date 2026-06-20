# BeatFixtureKit — Swift Package Design Plan

## Purpose

`BeatFixtureKit` is a lightweight Swift Package that lets any iOS/macOS music application:

1. **Load** a `test-fixture.json` file produced by `onset-matcher`
2. **Record** MIDI note-on events emitted by the app's beat engine during a test run
3. **Compare** the recorded output against the fixture
4. **Report** pass/fail with human-readable diagnostics

The package has no dependency on `onset-matcher` at runtime. It only depends on the `test-fixture.json` schema, which is stable and language-agnostic.

---

## Repository

Suggested name: `BeatFixtureKit` (separate Swift Package repository, not inside `onset-matcher`).

```
BeatFixtureKit/
  Package.swift
  Sources/
    BeatFixtureKit/
      Models.swift            — Codable types matching test-fixture.json
      BeatEventRecorder.swift — Captures (beat, note) pairs from a beat engine
      BeatFixtureMatcher.swift — Compares captured events against a fixture
      MatchResult.swift       — Result types with diagnostics
  Tests/
    BeatFixtureKitTests/
      BeatFixtureMatcherTests.swift
```

---

## Module: Models

```swift
/// The top-level fixture loaded from test-fixture.json.
public struct BeatFixture: Codable, Sendable {
    public let schemaVersion: String
    public let name: String
    public let bpm: Double
    public let timeSignature: [Int]   // [numerator, denominator]
    public let events: [BeatCluster]

    public static func load(from url: URL) throws -> BeatFixture
    public static func load(fromJSON data: Data) throws -> BeatFixture
}

/// One beat cluster: a group of notes that should fire at the same beat.
public struct BeatCluster: Codable, Sendable {
    public let beat: Double
    public let notes: [Int]   // sorted, deduplicated MIDI note numbers
}
```

Loading example:
```swift
let fixture = try BeatFixture.load(from: Bundle.test.url(forResource: "kick_loop", withExtension: "json")!)
```

---

## Module: BeatEventRecorder

The recorder is attached to a beat engine during a test. It collects raw `(beat, note)` events and then groups them into clusters for comparison.

```swift
/// Protocol that a beat engine adopts to notify observers of note events.
/// The beat engine only needs to call one method — it has no knowledge of testing.
public protocol BeatEngineObserving: AnyObject {
    func beatEngine(
        _ engine: any BeatEngineProtocol,
        didFireNote note: Int,
        atBeat beat: Double
    )
}

/// Records MIDI note-on events for later comparison against a fixture.
public final class BeatEventRecorder: BeatEngineObserving, Sendable {

    /// Clustering tolerance: events within this many beats are merged into one cluster.
    /// Default: 0.05 beats.
    public let clusteringTolerance: Double

    public init(clusteringTolerance: Double = 0.05)

    /// Called by the beat engine for each note-on event.
    public func beatEngine(_ engine: any BeatEngineProtocol, didFireNote note: Int, atBeat beat: Double)

    /// All captured events as raw (beat, note) pairs, in arrival order.
    public var capturedEvents: [(beat: Double, note: Int)] { get }

    /// Captured events grouped into beat clusters (sorted by beat).
    public var capturedClusters: [BeatCluster] { get }

    /// Remove all captured events (call before starting a new test run).
    public func reset()
}
```

### Observer attachment pattern

The beat engine exposes a weak observer list:

```swift
protocol BeatEngineProtocol: AnyObject {
    func addObserver(_ observer: any BeatEngineObserving)
    func removeObserver(_ observer: any BeatEngineObserving)
}
```

In a test:

```swift
let recorder = BeatEventRecorder()
engine.addObserver(recorder)
engine.play()
// ... wait for the pattern to finish ...
engine.stop()
engine.removeObserver(recorder)

let result = BeatFixtureMatcher().match(recorded: recorder.capturedClusters, against: fixture)
```

---

## Module: BeatFixtureMatcher

```swift
/// Configuration for the comparison.
public struct MatcherConfig: Sendable {
    /// Maximum beat distance for a captured cluster to count as matching an expected cluster.
    /// Default: 0.05 beats.
    public var beatTolerance: Double

    /// If true, captured clusters with no matching expected cluster are reported as unexpected events.
    /// Default: true.
    public var reportUnexpected: Bool

    public init(beatTolerance: Double = 0.05, reportUnexpected: Bool = true)
}

/// Compares a list of captured beat clusters against a fixture.
public struct BeatFixtureMatcher: Sendable {

    public let config: MatcherConfig

    public init(config: MatcherConfig = MatcherConfig())

    /// Run the comparison and return a detailed result.
    public func match(
        recorded: [BeatCluster],
        against fixture: BeatFixture
    ) -> BeatFixtureMatchResult
}
```

### Matching algorithm

For each expected cluster (in beat order):
1. Find the closest captured cluster within `beatTolerance`.
2. If none found → **missing event**.
3. If found but notes differ → **wrong notes**.
4. If found and notes match → **matched** (record timing error = captured.beat − expected.beat).

After matching all expected clusters, any remaining captured clusters are **unexpected events** (if `reportUnexpected` is enabled).

---

## Module: MatchResult

```swift
public struct BeatFixtureMatchResult: Sendable {

    /// True if all expected clusters were matched with correct notes.
    public let passed: Bool

    /// Number of expected clusters that were matched correctly.
    public let matchedCount: Int

    /// Total number of expected clusters in the fixture.
    public let totalExpected: Int

    /// Coverage ratio: matchedCount / totalExpected.  [0.0, 1.0]
    public let coverage: Double

    /// Mean absolute beat timing error of matched clusters (beats).
    public let meanTimingError: Double

    /// Expected clusters that had no matching captured cluster.
    public let missingEvents: [BeatCluster]

    /// Expected clusters that matched by beat but had wrong notes.
    public let wrongNoteEvents: [WrongNoteEvent]

    /// Captured clusters with no matching expected cluster.
    public let unexpectedEvents: [BeatCluster]

    /// Per-cluster timing details for matched pairs.
    public let timingDetails: [ClusterTimingDetail]

    /// Human-readable summary string (suitable for XCTest failure messages).
    public var summary: String { get }
}

public struct WrongNoteEvent: Sendable {
    public let expectedBeat: Double
    public let expectedNotes: [Int]
    public let capturedBeat: Double
    public let capturedNotes: [Int]
    public let timingError: Double
}

public struct ClusterTimingDetail: Sendable {
    public let expectedBeat: Double
    public let capturedBeat: Double
    public let timingError: Double    // captured - expected, signed
}
```

---

## XCTest integration

```swift
import XCTest
import BeatFixtureKit

class MyEngineTests: XCTestCase {

    var engine: MyBeatEngine!

    override func setUp() {
        engine = MyBeatEngine()
    }

    func testKickLoopPattern() throws {
        // 1. Load fixture
        let fixtureURL = Bundle.module.url(forResource: "kick_loop", withExtension: "json")!
        let fixture = try BeatFixture.load(from: fixtureURL)

        // 2. Record
        let recorder = BeatEventRecorder(clusteringTolerance: 0.05)
        engine.addObserver(recorder)
        engine.play()
        // Give the engine enough time to emit all events (or use a semaphore/expectation)
        Thread.sleep(forTimeInterval: fixture.durationSeconds + 0.5)
        engine.stop()
        engine.removeObserver(recorder)

        // 3. Compare
        let result = BeatFixtureMatcher().match(
            recorded: recorder.capturedClusters,
            against: fixture
        )

        // 4. Assert
        XCTAssertTrue(result.passed, result.summary)
    }
}
```

---

## Design decisions

| Decision | Choice | Rationale |
|---|---|---|
| Observer pattern vs. polling | Observer protocol | Beat engine doesn't need to know about testing; recorder is only attached during tests |
| Clustering in recorder | Yes, at `clusteringTolerance` | Notes fired very close together (within one tick) should form one cluster before comparison |
| Velocity in comparison | Not included | Velocity is instrument-dependent; the fixture intentionally omits it |
| Note ordering in clusters | Sorted, deduplicated | Order is irrelevant; duplicates from repeated note-on are deduped |
| Beat zero convention | 0-based (beat 0 = start of pattern) | Matches `onset-matcher` output; apps counting from beat 1 must subtract 1 |
| Beat tolerance source | Caller-supplied, default 0.05 | Different apps have different scheduling precision; caller knows best |
| Async support | Not in v1 | Beat engines are typically synchronous in unit tests; async variant can be added later |
| Sendable conformance | Yes | Swift concurrency safety; recorder uses an actor or lock internally |

---

## Open questions for implementation

1. **Recorder thread safety**: Beat engines may fire events on a background thread (CoreMIDI callback). The recorder's internal state needs a lock or actor. Simplest: use `NSLock` around the append; or make `BeatEventRecorder` an `actor`.

2. **Beat zero alignment**: If the app starts playback at beat 1 (bar 1, beat 1) the test must account for the offset. Should `BeatFixtureMatcher` accept a `beatOffset` parameter, or should the caller normalize? Recommendation: add `capturedBeatOffset: Double = 0.0` to `match(recorded:against:)`.

3. **Pattern looping**: If the engine loops the pattern, captured events will repeat. The matcher should either be told the expected loop count, or it should only compare the first `N` beats (where `N` is the fixture's duration). Simplest: add `maxBeat: Double? = nil` to `MatcherConfig`.

4. **`BeatFixture.durationSeconds`**: A convenience computed property derived from `bpm` and `events.last?.beat`. Useful for waiting the right amount of time in tests.

5. **Strict vs. lenient mode**: Should the test fail if the app fires *extra* notes not in the fixture (e.g. a fill that shouldn't be there)? Controlled by `reportUnexpected` in `MatcherConfig`.

---

## Relationship to onset-matcher

```
onset-matcher (Rust CLI)
    ↓  produces
test-fixture.json
    ↓  loaded by
BeatFixtureKit (Swift Package)
    ↓  used in
XCTest unit tests in your iOS/macOS music app
```

`BeatFixtureKit` does not depend on `onset-matcher` at runtime. The only coupling is the JSON schema (`schemaVersion: "0.1"`). When the schema changes, update the `Codable` models and bump the version check.
