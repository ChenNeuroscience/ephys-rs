# Development

## Building and running

```bash
cargo run --release                            # an empty graph
cargo run --release -- examples/rig.json       # the example graph
```

Debug builds compile the app without optimisation but its dependencies with it (egui and the rest are many times
slower otherwise), so they're usable; release builds are what to measure performance with.

Without hardware, [`synthetic_streams/`](../synthetic_streams) has a fake acquisition board and a fake camera (Python,
standard library only), which nodes use by default. Use the "Use synthetic data" quick action to switch a graph to
them.

## Testing

```bash
cargo test                       # the engine, nodes, and the GUI rendered headless
UPDATE_SNAPSHOTS=1 cargo test    # accept changes to how the GUI looks
cargo test -- --ignored          # also the end-to-end tests against synthetic_streams (needs python3)
```

GUI tests render with `egui_kittest` (on wgpu) and compare against images in
[`tests/snapshots`](../tests/snapshots), which also serve as these docs' screenshots. They're made on macOS; Windows
gets some leeway ([`kittest.toml`](../kittest.toml)). Snapshots must look the same every run, so tests freeze what
varies: the frame timings are paused, the worker count set, the version shown as "v0.x", and the day (for subjects' trials)
always 2026-01-01.

Tests that need a board or camera use in-process stand-ins (`serve_test_board`, `serve_test_scene`) rather than the
Python ones, and tests that record remove the writers first, or write to a temporary directory: never to
`~/recordings`.

CI ([`.github/workflows/build.yml`](../.github/workflows/build.yml)) checks formatting and clippy (warnings are
errors), runs the tests on Windows and macOS, and builds release packages. Each push to `main` publishes a release,
named after the version (`v0.x`, below).

## Versions

The version is `v0.x`, shown at the right of the status bar: `x` is Cargo.toml's minor version, a count of changes
(not semver). Every commit counts it up, by a pre-commit hook, [`.githooks/pre-commit`](../.githooks/pre-commit),
which bumps Cargo.toml and Cargo.lock and stages them. Turn it on once per clone:

```bash
git config core.hooksPath .githooks
```

For now, files and the clipboard are read only as they're written now: nothing is kept to read older versions.

## Performance

The dev tools (Ctrl/⌘ Alt I) show each step's time in the passes (its work, and any wait at a sync point), the passes'
and the GUI's frames over time, and the buffers and pools. The charts mark changes to the run (new epochs, versions,
recording) twice: a dashed line when the change was asked for, and a solid one at the first pass to use it
(`Runner::first_uses`). Hovering either says what changed, and the solid one how long it took: a pass or so for new
settings, longer for a new epoch, as passes carry on with the old one until a worker's ready for it.
Release builds keep line tables, so profilers and crash backtraces name lines.

## The README's tour

`scripts/make_tour.sh` remakes `assets/tour.gif`: the real app, driven on synthetic data, a frame for each feature with
a caption. To change what's in it, edit [`tour.rs`](../src/core/gui/tests/tour.rs): each `shot` is a frame. It's an
ignored test only because the test harness is what draws the app without a window. It starts its own fake board and
camera, on ports of their own, and writes nothing but the GIF, and the frames in `target/tour`. As in tests, the date,
the version and the disk's free space are fixed.

## Crash reports

A crash (a panic anywhere, or on Windows a crash in native code, such as a device's library) writes a report beside
the app's preferences (`crashes/`, under eframe's storage directory: on Windows, `%APPDATA%\ephys-rs\data`), and ends
the app; the next start says where. The report has the thread, the pass and node it was running, a backtrace, and the
log's last lines. To try it, start with `EPHYS_RS_TEST_CRASH=panic` (or `native`), which crashes after two seconds.

## Writing code here

- Read the module docs first: each says what it's for and how it works.
- Comments say what and why, in plain words; names are full words.
- Prefer a standard crate to rolling our own.
- Nothing is allocated per pass: make buffers before running, and reuse them.
- Settings changes go through snapshots; passes never wait on the GUI, nor it on them (see
  [Architecture](architecture.md)).
