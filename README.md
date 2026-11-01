<img src="assets/icon.svg" width="96" height="96" alt="Three blue, green and red spikes on a black background">

# ephys-rs
Electrophysiology software derived from [Open Ephys](https://github.com/open-ephys), but rewritten in Rust with a custom built execution engine and more flexible user interface.


## Status

Development of this project started on 2026-Oct-31. With the help of today's AI, progress has been incredibly rapid. No serious testing has been done yet though and there are lots of things still to be done.

## What's there

You build a graph of nodes, and press Play to run it:

- **Sources**: the Acquisition Board (Open Ephys's boards, generations 1 to 3, and Intan's RHD USB board, or the fake one below), with headstage channels, auxiliary and analogue inputs, and digital inputs as events; and the Camera (FLIR cameras through Spinnaker, or the fake one).
- **Processing**: Reference (one channel, common average or median, per group, or per channel), Band-pass Filter, Gain, Spike Detector (in tetrodes, or groups of any size), and Position Detector (LEDs in video).
- **Viewers**: LFP, spikes, video, and the Tracker View (the camera, with detection and calibration).
- **Writers**: Write LTX (tetrode spikes, 5 kHz continuous data, positions and digital inputs, for legacy tetrode analysis code) and Write Raw (Open Ephys's binary format). Press Record (or Ctrl/⌘ R): the writers open their files, then recording starts, and the acquisition clock beside Play shows how long it's been going.

Nodes can also edit each other's settings, through settings edges (dashed); a node that only reads a setting, as Write LTX reads the position detector's crop and calibration to record them, has a dotted edge.

Developer Tools (View menu, or Ctrl/⌘ Alt I) show the threads running the graph, the plan each pass runs through and the memory it takes, and the log.

## Downloads and development

Each push to `main` is tested and built on Windows and macOS by GitHub Actions, and published as a [release](https://github.com/ChenNeuroscience/ephys-rs/releases) named after the commit. On macOS the app isn't notarised, so the first time, right-click it and choose Open.

```bash
cargo run --release                            # starts with an empty graph
cargo run --release -- examples/rig.json       # opens a graph file: here, the example
cargo test                                     # includes GUI tests, rendered headless against tests/snapshots
UPDATE_SNAPSHOTS=1 cargo test                  # accept changes to how the GUI looks
```

The code is in two parts: `src/core` (the graph engine, the GUI, and the traits for annotations and themes), which knows nothing about particular node, edge or annotation types; and `src/standard_components`, grouped by feature (`ephys`, `spikes`, `tracking`...), each with its own data stream, settings and node types. `src/main.rs` puts the two together, declaring which components the program is built with.

The icon is `assets/icon.svg`; `scripts/export_icon.js` exports the PNGs from it.

Without real hardware, `synthetic_streams/` has stand-ins, which nodes use by default (the example's included):

- `python3 synthetic_streams/fake_acquisition_board.py` streams an acquisition board's headstage channels (tetrodes picking up spiking units over theta LFP), accelerometers, analogue inputs and digital inputs.
- `python3 synthetic_streams/fake_camera.py` streams video of a mouse with two head LEDs in an arena.
