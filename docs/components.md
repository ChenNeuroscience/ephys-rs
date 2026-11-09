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
