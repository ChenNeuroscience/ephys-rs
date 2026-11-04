<img src="assets/icon.svg" width="96" height="96" alt="Three blue, green and red spikes on a black background">

# ephys-rs
Electrophysiology software derived from [Open Ephys](https://github.com/open-ephys), but rewritten in Rust with a custom built execution engine and more flexible user interface.


> [!WARNING]
> ephys-rs is now in alpha testing, for use with [LTX format](https://github.com/ChenNeuroscience/ltx-format) recordings. Development started on 2026-Oct-31 and has moved quickly with the help of AI, under human supervision.

## A quick tour

![A tour of ephys-rs: live spikes, LFP and tracking, recording, the graph, and the developer tools](assets/tour.gif)

Start from a quick action, or build your own graph of sources, processing, viewers and writers, then press Play. It acquires from Open Ephys and Intan boards and FLIR cameras, and records [LTX format](https://github.com/ChenNeuroscience/ltx-format) or Open Ephys binary.

## Downloads

Each push to `main` is tested and built on Windows and macOS by GitHub Actions, and published as a [release](https://github.com/ChenNeuroscience/ephys-rs/releases). On macOS the app isn't notarised, so the first time, right-click it and choose Open.


## Developing

```bash
cargo run --release                            # the example program, starting with an empty graph
cargo test                                     # includes GUI tests, rendered headless against tests/snapshots
UPDATE_SNAPSHOTS=1 cargo test                  # accept changes to how the GUI looks
```

**[Developer docs](docs/README.md)** describes the architecture (the graph, plans, passes and epochs, threads and buffers), and how to write nodes, edges, annotations and quick actions. Turn on the hook that counts the versions up with each commit: `git config core.hooksPath .githooks`.

The code is in three parts:

1. `src/core`: the graph engine, the GUI, and the traits for nodes, annotations and themes. It knows nothing about particular node, edge or annotation types.
2. `src/standard_components`: the components most labs will need, grouped by feature (`ephys`, `spikes`, `tracking`...).
3. `example/`: a custom build, and what this repository releases. It declares which components it's built with, and adds its own: the LTX quick actions, and the subject, date and trial base directory.

The first two are the `ephys-rs` library. To make your own build, add it to a new program with `cargo add --git https://github.com/ChenNeuroscience/ephys-rs ephys-rs`, and use `example/` as a starting point: [custom builds](docs/architecture.md#core-standard-components-and-custom-builds) says what you can add.

To run without real hardware, `synthetic_streams/` has stand-ins, which nodes use by default:

- `python3 synthetic_streams/fake_acquisition_board.py` streams an acquisition board's headstage channels (tetrodes picking up spiking units over theta LFP), accelerometers, analogue inputs and digital inputs.
- `python3 synthetic_streams/fake_camera.py` streams video of a mouse with two head LEDs in an arena.
