# Development

## Building and running

```bash
cargo run --release                            # chen-neuro, with an empty graph
cargo run --release -- my-graph.json           # opening a graph file
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
varies: the frame timings are paused, the worker count set, plans' addresses fixed, the version shown as "chen-neuro v0.x (ephys-rs v0.y)", and the day (for subjects' trials)
always 2026-01-01.

Tests that need a board or camera use in-process stand-ins (`serve_test_board`, `serve_test_scene`) rather than the
Python ones, and tests that record remove the writers first, or write to a temporary directory: never to
`~/recordings`.

CI ([`.github/workflows/build.yml`](../.github/workflows/build.yml)) checks formatting and clippy (warnings are
errors), runs the tests on Windows and macOS, and builds release packages for both. Each push to `main`, other than of
Markdown alone, publishes a release, titled with the whole version (below), which one of the builds is asked for with
`--version`. It's tagged `chen-neuro-v0.x`, and the first with a new core version is tagged `ephys-rs-v0.y` too (tags
can't have spaces). Releases before these two versions were split are tagged `v0.x`.

## Cameras

The console says how a camera's getting on: what's being looked for, where the Spinnaker SDK was loaded from, which
cameras it sees, how the one opened is set up (its size, pixel format, trigger mode, frame rate, exposure and gain), and
the size of the first frame. Connecting's tried twice a second, so each of those is said once. It warns if frames are
incomplete, or stop coming, or aren't the size set.

The video stream's size has to be known before running, for its buffers, so it's a setting: the Camera node's Frame
size, one of its camera settings. Frames of any other size are skipped. When they come, the camera proposes the size
they are (`Pass::propose`, like other auto-detected settings), which starts a new epoch.

A new epoch doesn't open the camera again, as that takes a couple of seconds: it carries on with the one capturing, only
changing the buffers if the frame size has, and setting the frame rate on it if that has (a triggered camera goes by its
trigger, so there it only changes the rate declared for the video). Stopping closes it, so other programs can use it, and
changing its source opens another. See [`capture.rs`](../src/standard_components/tracking/camera/capture.rs).

While recording, settings are locked, so it logs an error instead, and proposes the size once recording stops.

## The timing LED

A Position Detector can watch a spot of its own for a timing LED, set by `Detection::timing_led`: usually outside the
crop, and placed by dragging a circle over the Tracker View's video. It's on when enough of the spot is bright enough,
by the other LEDs' threshold and min area, or its own. The spot isn't looked at for the other LEDs. See
[`pipeline.rs`](../src/standard_components/tracking/pipeline.rs).

Its changes come out of the detector's `pos ttl` output as ordinary `Events`, one line, ups and downs, so anything that
takes TTLs can take them. Each carries the camera's time (`Event::time`), as positions do. Write LTX puts them in
`.pos.ttl`, counting from the first position, and only makes the file if there's a timing LED.

Telling a change means remembering the last frame, so the detector's state is carried over, and it's a sync point.

The Tracker View's LED sizes chart shows when it was on as pale blue bars, whether or not its section's hidden: hiding
only takes it off the video.

## Alarms

The [Alarm Detector](../src/standard_components/alarms/detector.rs) sits with the data, and the
[Alarms viewer](../src/standard_components/alarms/viewer.rs) only shows it: the viewer edits the detector's settings
through a control, and gets back a few numbers a pass (`AlarmStates`), so nothing bulky goes to the window. Each alarm
counts in bins, a fiftieth of its period each, and doesn't go off until it's seen a whole period, so a new period, or
epoch, starts it waiting again. Time is counted in passes, not samples, so a source that stops looks like a low rate.

An alarm that's off isn't needed (`Node::needs`), so switching one changes the plan, and starts a new epoch. While
recording the graph can't change, so the viewer only ignores what it's switched off, and saves that in the settings
once recording's stopped, if its window's open by then (otherwise it's lost when the run stops). An alarm off when
recording started has no data coming, so it can't come on until then.

The tone is the viewer's own, on a thread started the first time it sounds, from
[`tone.rs`](../src/standard_components/alarms/tone.rs) with `cpal`. It's started and stopped by passes, so it works with
the window closed, and needs nothing from the core. On Linux, `cpal` needs ALSA's headers (`libasound2-dev`) to build.

## Errors and warnings

The status bar counts the errors and warnings logged so far this session, whatever logged them, and clicking the count
opens the console. Nothing's counted until there's something to count, and the totals don't go back down. Tests share
one log, so they fix what's shown (`App::problems`), rather than showing each other's.

## Versions

There are two versions, shown together at the right of the status bar: `chen-neuro v0.x (ephys-rs v0.y)`. A build says
what it shows, with `Components::version` (see [custom builds](architecture.md#core-standard-components-and-custom-builds)),
and this one's in [`examples/chen-neuro/`](../examples/chen-neuro/src/components/mod.rs).

- `x` is the build's: examples/chen-neuro/Cargo.toml's minor version, from 900. It's what's released, so every commit counts it up,
  including those that only change the core. Commits of nothing but Markdown (the README, these docs) count up neither.
- `y` is the core's: the root Cargo.toml's minor version. Only commits that change the library (`src/`, or the root
  Cargo.toml) count it up.

Both are counts of changes (not semver). A pre-commit hook, [`.githooks/pre-commit`](../.githooks/pre-commit), counts
them, bumping the Cargo.toml files and Cargo.lock and staging them. Turn it on once per clone:

```bash
git config core.hooksPath .githooks
```

Anything can ask for them: `core::full_version()` is the whole thing, and `core::version()` the core's alone. Write LTX
keeps the whole thing in its headers (`created_by`, `data_source`), Write Raw in `structure.oebin` (`recorded_with`), and
crash reports start with it.

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
the versions and the disk's free space are fixed.

## The icon

`scripts/export_icon.js` makes the icons from `assets/icon.svg`. In `icon.ico`, only the 256 pixel image is a PNG, and
the smaller ones are bitmaps, as Windows doesn't reliably read PNGs at small sizes.

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
