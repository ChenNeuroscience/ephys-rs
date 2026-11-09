# Components

How some of the components work, beyond what their code's comments say: what to keep in mind when changing them. For
how they're used, see the [user guide](user/README.md).

## Cameras

The console says how a camera's getting on: what's being looked for, where the Spinnaker SDK was loaded from, which
cameras it sees, how the one opened is set up (its size, pixel format, trigger mode, frame rate, exposure and gain), and
the size of the first frame. Connecting's tried twice a second, so each of those is said once. It warns if frames are
incomplete, or stop coming, or aren't the size set.

The video stream's size has to be known before running, for its buffers, so it's a setting: the Camera node's Frame
size, one of its camera settings. Frames of any other size are skipped. When they come, the camera triggers an update to the size
they are (`Pass::trigger_settings_update`, like other auto-detected settings), which starts a new epoch.

A new epoch doesn't open the camera again, as that takes a couple of seconds: it carries on with the one capturing, only
changing the buffers if the frame size has, and setting the frame rate on it if that has (a triggered camera goes by its
trigger, so there it only changes the rate declared for the video). Stopping closes it, so other programs can use it, and
changing its source opens another. See [`capture.rs`](../src/standard_components/tracking/camera/capture.rs).

While recording, settings are locked, so it logs an error instead, and triggers the update once recording stops.

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

## The VR Computer

chen-neuro's [VR Computer](../examples/chen-neuro/src/components/vr_computer.rs) node is an alternative to the camera, for
a virtual reality task running on another computer. The "Use VR computer" and "Use camera" quick actions swap between
them. It connects to the IP address and port it's set to, and keeps trying. What it reads is described in the
[VR computer protocol](../examples/chen-neuro/VR_COMPUTER_PROTOCOL.md), which is for whoever's writing the VR side: keep it up
to date with any changes here. This is how it's handled.

The VR computer is expected to send its settings on being connected to, and whenever they change: nothing asks for them.
The VR computer sends positions as a camera would, in pixels of a made-up frame, so that a recording suits whatever reads a
camera's, and nothing here scales them. So settings have to have the four `window_*` values, the window positions range
over (each min below its max), `pixels_per_metre`, and `timing_led.enabled`, each named as a camera's are: otherwise they're refused, with an error logged, and the node warns
until it's had them. To keep full resolution, the window needs to span about 1022 pixels; any more is scaled down, as
a camera's frame is.
Nested keys are joined with dots (`maze.arms`), and they reach Write LTX through `Detection::custom`, on the node's
setting, connected to Write LTX's `detection` control. They're written to the `.pos` file as headers of their own, keys as
they are: one could have a space in it, or be the same as another header, which is for the reader to make sense of. The
exceptions are the required values above, which aren't repeated, as they reach Write LTX as settings of its own.

The window goes to Write LTX as `Detection::crop`, a rectangle, and the calibration as a ruler a metre long, so they're
handled as a camera's are, in the `.pos` file's `window_*`, `position_coordinate_*` and `pixels_per_metre` headers. `timing_led.enabled`
goes as `Detection::timing_led`, with a radius of 0 as it has no spot in any video. Write LTX has no code
for any of this: it makes `.pos.ttl` as for a camera's timing LED, so the file's `ttl_source` is `timing_led`, and its
`timing_led_*` headers are the defaults, with the radius 0 saying there's no real spot.

Headers are written as recording starts, so settings can't change while recording. Ones that differ log an error and wait:
only the latest is kept, in the connection's inbox, so it survives a new epoch, and the update's triggered once recording stops,
until the node has it. So a connection's last settings aren't lost, though the recording has the ones it started with.

`time` is required on all but `settings`, in seconds by the VR computer's own clock, which has to be consistent for as long
as it's connected. It's carried as it came all the way downstream, and when messages arrived plays no part: `PositionSample::time`,
`Event::time` and each log line's time are the VR computer's values. Only Write LTX moves zero, to the first position
recorded, for `.pos`, `.pos.ttl` and `.txt` alike, so the three can be compared; they have no known relation to the
board's samples. The `.txt` file has no headers, and each line is `<seconds> <line>`.

The Tracker View stays on, with the VR Computer's positions and setting connected in place of the detector's. With no
video input it shows the current position and the recording's path in the window, and the settings, only to read: there's
nothing to adjust, so it doesn't need telling it's locked, and it edits nothing. It looks as it does with a camera as far
as it can: the window's drawn as the crop, the position as the first LED, and the LED sizes chart shows the dummy size.

The [Log Viewer](../src/standard_components/logs/log_viewer.rs) shows the log lines with their times, as the VR computer
sent them, so not counted from anything: it's added and removed with the VR Computer by the quick actions.

`Positions::source` says where positions come from, and is `position_metadata_source` in the `.pos` file, and the prefix of
its other source-specific headers (`vr_computer.window`). The second LED is always missing, and the first has a size of 1.

## Alarms

Alarms are added one at a time in the [Alarm Processor](../src/standard_components/alarms/processor.rs)'s window, from
a library of kinds: `AlarmKind` has a variant for each, holding the settings it needs, so a new kind is a variant there
(and in `AlarmKind::library`), and the `match`es the compiler then points to. There can be several kinds for one input,
and several of one kind; only the kinds with their input connected are offered. Most are evaluated over a period, but
one with `AlarmKind::has_period` false goes by what's happening now: the TTL up alarm follows its line's changes, so it
takes a line to be down until it's seen it change. An alarm has an `id` that isn't reused, and states are matched to
alarms by it. The stream
(`AlarmStates`) is only what there is to choose from, so adding an alarm doesn't change it; how they stand comes each
pass (`AlarmStatesBuffer`), a few numbers for each alarm.

The processor sits with the data. The [Alarm Viewer](../src/standard_components/alarms/viewer.rs) shows how the alarms
stand, and has their on and off switches, which edit the processor's settings through a control, so nothing bulky goes to
the window. Each alarm counts in bins, a fiftieth of its period each, and doesn't go off until it's
seen a whole period, so a new period, or epoch, starts it waiting again. Time is counted in passes, not samples, so a
source that stops looks like a low rate.

The buttons for all the alarms are only shortcuts, setting each one, so there's no separate on or off to go out of step.

An input's only needed while an alarm that's on watches it (`Node::evaluate_tree_shake`), so switching the last one off changes the
plan, and starts a new epoch. While
recording the graph can't change, so the viewer only ignores what it's switched off, and saves that in the settings
once recording's stopped, if its window's open by then (otherwise it's lost when the run stops). An alarm off when
recording started has no data coming, so it can't come on until then.

The pips are the viewer's own, on a thread started the first time they sound, from
[`tone.rs`](../src/standard_components/alarms/tone.rs) with `cpal`: a couple as an alarm goes off, at a pitch for what it
watches, and more if it's still going off after 30 and 60 s, then no more. They only sound while recording: until then,
the viewer says so if any alarm has sound on, and an alarm going off as recording starts pips then. The viewer times how long each
has been going off, by adding up the passes, and shows it on the alarm. A change to the plan starts that again, as it
does the alarm itself. Which alarms pip is in the viewer's own settings. Passes
start them, so they work with the window closed, and they need nothing from the core. On Linux, `cpal` needs ALSA's
headers (`libasound2-dev`) to build.
