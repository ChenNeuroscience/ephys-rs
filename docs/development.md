# Development

## Building and running

```bash
cargo run --release                            # chen-neuro, with an empty graph
cargo run --release -- my-graph.json           # opening a graph file
```

Debug builds compile the app without optimisation but its dependencies with it (egui and the rest are many times
slower otherwise), so they're usable; release builds are what to measure performance with.

Without hardware, [`synthetic_streams/`](../synthetic_streams) has a fake acquisition board, a fake camera and a fake VR
computer (Python, standard library only), which nodes use by default. Use the "Use synthetic data" quick action to switch a graph to
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
varies: the frame timings are frozen, the worker count set, plans' addresses fixed, the version shown as "chen-neuro v0.x (ephys-rs v0.y)", and the day (for subjects' trials)
always 2026-01-01.

Tests that need a board or camera use in-process stand-ins (`serve_test_board`, `serve_test_scene`) rather than the
Python ones, and tests that record remove the writers first, or write to a temporary directory: never to
`~/recordings`.

CI ([`.github/workflows/build.yml`](../.github/workflows/build.yml)) checks formatting and clippy (warnings are
errors), runs the tests on Windows and macOS, and builds release packages for both. Each push to `main`, other than of
Markdown alone, publishes a release, titled with the whole version (below), which one of the builds is asked for with
`--version`. It's tagged `chen-neuro-v0.x`, and the first with a new core version is tagged `ephys-rs-v0.y` too (tags
can't have spaces). Releases before these two versions were split are tagged `v0.x`.

## Errors and warnings

The status bar counts the errors and warnings logged so far this session, whatever logged them, and clicking the count
opens the console. Nothing's counted until there's something to count, and the totals don't go back down. Tests share
one log, so they fix what's shown (`App::problems`), rather than showing each other's.

## Versions

There are two versions, shown together at the right of the status bar: `chen-neuro v0.x (ephys-rs v0.y)`. A build says
what it shows, with `Components::version` (see [custom builds](architecture.md#the-framework-standard-components-and-custom-builds)),
and this one's in [`examples/chen-neuro/`](../examples/chen-neuro/src/components/mod.rs).

- `x` is the build's: examples/chen-neuro/Cargo.toml's minor version, from 900. It's what's released, so every commit counts it up,
  including those that only change the core. Commits of nothing but Markdown (the README, these docs) count up neither.
- `y` is the core's: the minor version of the workspace's, in the root Cargo.toml, which the framework and the
  standard components both have. Only commits that change them (`src/`, `standard/`, or the root Cargo.toml) count it
  up.

Both are counts of changes (not semver). A pre-commit hook, [`.githooks/pre-commit`](../.githooks/pre-commit), counts
them, bumping the Cargo.toml files and Cargo.lock and staging them. Turn it on once per clone:

```bash
git config core.hooksPath .githooks
```

Anything can ask for them: `ephys_rs::full_version()` is the whole thing, and `ephys_rs::version()` the library's alone. Write LTX
keeps the whole thing in its headers (`created_by`, `data_source`), Write Raw in `structure.oebin` (`recorded_with`), and
crash reports start with it.

## Config patches

All the config the app reads back says which version of each part of the program wrote it: saved files (projects, and
the settings saved beside recordings: `"versions"`), settings copied to the clipboard (`"ephys-rs settings"`), and the
base directory's settings in the app's stored preferences. Later versions patch it up to date before reading it. See
[`config_patching.rs`](../src/config_patching.rs).

Each part has a set of patches, saving its crate's version: the core's
([`config_patches`](../src/config_patches): the project's structure), the standard components'
([`standard/config_patches`](../standard/src/config_patches): their node types and base
directories), and a build's own (as [chen-neuro's](../examples/chen-neuro/src/components/config_patches)). Each is a
folder with a patch in each file, listed in order in its `mod.rs` as `PATCHES`, starting with an example that does
nothing (`example.rs`, to copy). Which sets a build has, and the order they're applied in, is up to it
(`Components::CONFIG_PATCHES`, as [chen-neuro's](../examples/chen-neuro/src/components/mod.rs) lists them: the core's,
the standard components', then its own).

To change what a part saves, add a patch to its set: a file with a function changing what was saved before to what's
saved now, as JSON (`serde_json::Value`), before anything's read into types, and add it to the end of `PATCHES`. Every
patch is applied to everything read, and whether it does anything is entirely up to it: think of it as a script more
than typical Rust. It's given config coming in (`IncomingConfig`): a saved file, settings pasted from the clipboard
(copied to lab notes, maybe, and back much later), or the base directory's settings from the app's preferences: the versions that wrote it (`config.written_before("std", "0.158.0")`
is the usual test: true too for what doesn't say), and the JSON, always shaped as a project. What isn't one (copied
nodes, the base directory's settings) is put in one for it, with nothing else. `config.nodes()` goes through its
nodes. So pasting is patched just as opening is. Test it with something saved before the change, as the patching tests do. A patch is never changed once
released.

## Performance

The dev tools (Ctrl/⌘ Alt I) show each step's time in the passes (its work, and any wait at a sync point), the passes'
and the GUI's frames over time, and the buffers and pools. The charts mark changes to the run (new epochs, versions,
recording) twice: a dashed line when the change was asked for, and a solid one at the first pass to use it
(`Runner::first_uses`). Hovering either says what changed, and the solid one how long it took: a pass or so for new
settings, longer for a new epoch, as passes carry on with the old one until a worker's ready for it.
Release builds keep line tables, so profilers and crash backtraces name lines.

## The README's tour

`scripts/make_tour.sh` remakes `assets/tour.gif`: the real app, driven on synthetic data, a frame for each feature with
a caption. To change what's in it, edit [`tour.rs`](../src/gui/tests/tour.rs): each `shot` is a frame. It's an
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
