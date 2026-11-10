# chen-neuro

The Chen lab's build of ephys-rs, and what this repository releases. It's the standard components, plus:

- **Quick actions** for the lab's rig, in the basic view: the standard LTX graph (an acquisition board and a FLIR camera, viewed and recorded by Write LTX, and an Arduino whose pin 8 is high while acquiring, which has the rig's sketch pulse pin 6 for syncing, once its port is chosen), swapping between the camera and a VR computer, and swapping between real devices and synthetic data.
- **VR Computer**, a source used instead of the camera for virtual reality tasks: positions, TTLs, log lines and settings, over TCP.
- **Subject, date and trial**, a base directory that files recordings by subject and day, and numbers the trials.
- **China**, a red and gold theme, for fun.

## Running it

From the root of the repository:

```bash
cargo run --release                     # with an empty graph
cargo run --release -- my-graph.json    # opening a graph file
```

## Trying the VR Computer without a VR computer

The stand-in script lives with the others, in [`synthetic_streams/`](../../synthetic_streams), so run it from the root of the repository too. It only needs Python 3.

1. Start the script, and leave it running:

   ```bash
   python3 synthetic_streams/fake_vr_computer.py
   ```

   It sends the fake camera's mouse as positions, with a TTL (on channel 1) and a log line every few seconds. Use `--port` and `--fps` to change where it listens and how often it sends positions.

2. In the app's basic view, run the **Standard LTX graph** quick action, then **Use VR computer**. This adds the VR Computer and a Log Viewer, which connects to the script's default port, and takes the camera (and the Arduino, which is for it) out of use.

3. Press Play. The Tracker View shows the mouse, and the Log Viewer its trials.

To see settings change, press Enter in the script's terminal. To put TTL channel 2 up or down, type `r` and Enter: add a
TTL Recording Control on its `pos ttl`, set to line 2, to start and stop recording by it. The script shows the state
ephys-rs sends back. **Use camera** swaps back.

What a real VR computer has to send is in the [VR computer protocol](VR_COMPUTER_PROTOCOL.md).

## How the VR Computer works

For whoever's changing it.

The [VR Computer](src/components/vr_computer.rs) node is an alternative to the camera, for
a virtual reality task running on another computer. The "Use VR computer" and "Use camera" quick actions swap between
them. It connects to the IP address and port it's set to, and keeps trying. What it reads is described in the
[VR computer protocol](VR_COMPUTER_PROTOCOL.md), which is for whoever's writing the VR side: keep it up
to date with any changes here. This is how it's handled.

The VR computer is expected to send its settings on being connected to, and whenever they change: nothing asks for them.
The VR computer sends positions as a camera would, in pixels of a made-up frame, so that a recording suits whatever reads a
camera's, and nothing here scales them. So settings have to have the four `window_*` values, the window positions range
over (each min below its max), `pixels_per_metre`, and `ttl_channels` (0 to 32), the first two named as a camera's are: otherwise they're refused, with an error logged, and the node warns
until it's had them. To keep full resolution, the window needs to span about 1022 pixels; any more is scaled down, as
a camera's frame is.
Nested keys are joined with dots (`maze.arms`), and they reach Write LTX through `Detection::custom`, on the node's
setting, connected to Write LTX's `detection` control. They're written to the `.pos` file as headers of their own, keys as
they are: one could have a space in it, or be the same as another header, which is for the reader to make sense of. The
exceptions are the required values above, which aren't repeated, as they reach Write LTX as settings of its own.

The window goes to Write LTX as `Detection::crop`, a rectangle, and the calibration as a ruler a metre long, so they're
handled as a camera's are, in the `.pos` file's `window_*`, `position_coordinate_*` and `pixels_per_metre` headers. Any
`ttl_channels` goes as `Detection::timing_led` enabled, with a radius of 0 as it has no spot in any video. Write LTX has no code
for any of this: it makes `.pos.ttl` as for a camera's timing LED, so the file's `ttl_source` is `timing_led`, and its
`timing_led_*` headers are the defaults, with the radius 0 saying there's no real spot.

Headers are written as recording starts, so settings can't change while recording. Ones that differ log an error and wait:
only the latest is kept, in the connection's inbox, so it survives a new epoch, and the update's triggered once recording stops,
until the node has it. So a connection's last settings aren't lost, though the recording has the ones it started with.

A `ttl`'s `idx` counts from 1, as the LTX files' `ttl_<n>` do, so it's the `pos ttl` stream's line `idx - 1`. Its
`Events::lines` is `ttl_channels`, and passes drop events on lines beyond it, logging an error once an epoch, so what's
downstream never sees a line the stream doesn't have.

It sends the VR computer its `state` too, on the connection's thread, which writes between reads: once a second, and as
soon as recording starts or stops, by `Pass::acquiring_for` and `Pass::recording_for` as the last pass saw them. A write
that times out stops them for the connection, as the VR computer isn't reading, rather than dropping it.

`time` is required on all but `settings`, in seconds by the VR computer's own clock, which has to be consistent for as long
as it's connected. It's carried as it came all the way downstream, and when messages arrived plays no part: `PositionSample::time`,
`Event::time` and each log line's time are the VR computer's values. Only Write LTX moves zero, to the first position
recorded, for `.pos`, `.pos.ttl` and `.txt` alike, so the three can be compared; they have no known relation to the
board's samples. The `.txt` file has no headers, and each line is `<seconds> <line>`.

The Tracker View stays on, with the VR Computer's positions and setting connected in place of the detector's. With no
video input it shows the current position and the recording's path in the window, and the settings, only to read: there's
nothing to adjust, so it doesn't need telling it's locked, and it edits nothing. It looks as it does with a camera as far
as it can: the window's drawn as the crop, the position as the first LED, and the LED sizes chart shows the dummy size.

The [Log Viewer](../../standard/src/logs/log_viewer.rs) shows the log lines with their times, as the VR computer
sent them, so not counted from anything: it's added and removed with the VR Computer by the quick actions.

`Positions::source` says where positions come from, and is `position_metadata_source` in the `.pos` file, and the prefix of
its other source-specific headers (`vr_computer.window`). The second LED is always missing, and the first has a size of 1.
