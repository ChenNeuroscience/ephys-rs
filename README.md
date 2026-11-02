<img src="assets/icon.svg" width="96" height="96" alt="Three blue, green and red spikes on a black background">

# ephys-rs
Electrophysiology software derived from [Open Ephys](https://github.com/open-ephys), but rewritten in Rust with a custom built execution engine and more flexible user interface.


## Status

Development of this project started on 2026-Oct-31. With the help of today's AI, progress has been incredibly rapid. No serious testing has been done yet though and there are lots of things still to be done.

## What's there

It opens on the **basic view**: the graph's viewers, large, to open beside it, and **quick actions** that build the graph for you (adding spike, EEG or position tracking, each recorded by Write LTX), reusing what's already there. The toolbar's **Home** brings back the home pane (which can be closed, once the windows you want are open); its menu switches it between the basic view and the **graph**, where you build it node by node (⌘1 / ⌘2), and whichever you pick is home from then on. Any node can be listed in the basic view (viewers are, to begin with).

You build a graph of nodes, and press Play to run it:

- **Sources**: the Acquisition Board (Open Ephys's boards, generations 1 to 3, and Intan's RHD USB board, or the fake one below), with headstage channels, auxiliary and analogue inputs, and digital inputs as events; and the Camera (FLIR cameras through Spinnaker, or the fake one).
- **Processing**: Channel Selector, Reference (one channel, common average or median, per group, or per channel), Band-pass Filter, Gain, Spike Detector (in tetrodes, or groups of any size, with each channel's threshold and gain), and Position Detector (LEDs in video).
- **Viewers**: LFP (with each channel's spectrum over a band, theta to begin with, and choosing a Channel Selector's channels), spikes (setting each channel's threshold and gain: by hand, or chosen from a few seconds of data), digital inputs (the TTL Viewer: each line's level, and how many times it's gone up since acquisition and recording started), video, and the Tracker View (the camera, with detection and calibration, and the path while recording).
- **Writers**: Write LTX (tetrode spikes, 5 kHz continuous data, positions and digital inputs, for legacy tetrode analysis code) and Write Raw (Open Ephys's binary format). Press Record (or Ctrl/⌘ R): the writers open their files, then recording starts, and the acquisition clock beside Play shows how long it's been going.

Nodes can also edit each other's settings, through settings edges (dashed, with arrows both ways); a node that only reads a setting, as Write LTX reads the spike detector's gains and the position detector's crop and calibration to record them, has a dotted edge. While recording, settings are locked.

Annotations sit on the canvas alongside the graph: sticky notes (plain text), markdown notes (with links that open in the browser, and images pasted into them), images (from a file, or pasted onto the canvas), headings, boxes to group things (behind everything, grabbed by their border or title), and arrows (thick and straight, so they aren't mistaken for edges). They're saved in the graph's file, images included.

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
