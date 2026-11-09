# User guide

How to use ephys-rs: setting up a rig, watching what's coming in, and recording it. For how it works under the hood,
see the [developer docs](../README.md).

> [!NOTE]
> The pictures here are of the Chen lab's build, [chen-neuro](../../examples/chen-neuro), running on synthetic data. Your
> lab's build may name things differently, and offer other quick actions, node types and ways of naming recordings. Only
> what comes with ephys-rs itself is described here. The pictures are the ones the tests check the GUI against, so they
> show it as it is now.

## Two ways to work

ephys-rs has two modes, switched with the toolbar's home and graph buttons:

- **[Basic mode](basic-mode.md)** is for running experiments. It shows the viewers (spikes, LFP, tracking...) as large
  buttons, and offers *quick actions*: ready-made setups, such as chen-neuro's "Standard LTX graph". You don't need to know
  about the graph to use it.
- **[Advanced mode](graph.md)** shows the *graph*: boxes (nodes) for each source, processing step, viewer and writer,
  and the wires (edges) between them. It's where you build or change a setup, and where everything can be adjusted.

The same setup runs either way: basic mode is just a simpler view of it.

## Contents

- [The main window](main-window.md): the toolbar, playing and recording, where recordings go, settings, and what to do
  when something goes wrong.
- [Basic mode](basic-mode.md): quick actions, and opening the viewers.
- [The graph](graph.md): nodes, ports and edges; adding and connecting nodes; settings edges; disabling; copying
  settings; windows; and notes on the canvas.
- Components, by what they're for:
  - [Electrophysiology](ephys.md): the Acquisition Board, Reference, Channel Selector, Band-pass Filter, Downsample,
    Continuous Gain and Spectrum; the LFP Viewer and TTL Viewer.
  - [Spikes](spikes.md): the Spike Detector, Spike Gain and Spike Viewer.
  - [Tracking](tracking.md): the Camera, Position Detector, Tracker View and Video Viewer.
  - [Alarms and logs](alarms-and-logs.md): the Alarm Processor and Alarm Viewer, and the Log Viewer.
  - [Recording](recording.md): Write LTX and Write Raw; Run Command, for processing what's recorded, and Sync
    Positions, for putting positions on the board's clock.
- [Keyboard shortcuts](main-window.md#keyboard-shortcuts)

## A typical session

1. Open a saved setup (File › Open, or a recent file from the home pane), or start from a quick action.
2. Choose where recordings go, in the toolbar (see [where recordings go](main-window.md#where-recordings-go)).
3. Press **Play** (Space) to start acquiring. Nothing is written yet: check the signals in the viewers, adjust
   thresholds and gains, and check the camera's tracking.
4. Press **Record** (Ctrl/⌘ R) to start recording, and again to stop.
5. Save the setup (Ctrl/⌘ S) if you changed it, so it's there next time.

> [!IMPORTANT]
> Streams from different sources aren't on a shared clock. Record TTLs alongside each one, and line them up afterwards:
> see [timing and synchronisation](../../README.md#timing-and-synchronisation).
