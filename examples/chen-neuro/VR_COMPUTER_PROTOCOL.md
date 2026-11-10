# VR computer protocol

How a computer running a virtual reality task sends its data to chen-neuro's VR Computer node, for recording alongside electrophysiology. If you're writing the VR side, this is all you need.

[`synthetic_streams/fake_vr_computer.py`](../../synthetic_streams/fake_vr_computer.py) is a small working example, which you can read or test against.

## In brief

- The VR computer **listens** on a TCP port, and ephys-rs connects to it.
- The VR computer sends its data, and ephys-rs sends back its `state` (how long it's been acquiring and recording),
  for the VR computer to show if it likes.
- Each message is one JSON object on one line, in UTF-8, ending with a newline (`\n`).
- As soon as ephys-rs connects, send `settings`. Send them again whenever they change.
- Send positions in made-up pixels, as if a camera were filming the animal.
- Every `pos`, `ttl` and `log` message has a `time`, from a clock of your own.
- TTL channels are numbered from 1 (`idx`).

```json
{"type": "settings", "settings": {"window_min_x": 0, "window_max_x": 1000, "window_min_y": 0, "window_max_y": 1000, "pixels_per_metre": 500, "ttl_channels": 2, "task": "linear"}}
{"type": "pos", "time": 1520.24, "x": 412.5, "y": 87.0}
{"type": "log", "time": 1520.25, "line": "trial 3 started"}
{"type": "ttl", "time": 1520.25, "idx": 1, "value": 1}
{"type": "pos", "time": 1520.26, "x": 413.1, "y": 88.2}
{"type": "ttl", "time": 1520.55, "idx": 1, "value": 0}
```

## The connection

The IP address and port to connect to are set on the VR Computer node. The port defaults to 27032.

ephys-rs connects when Play is pressed, and disconnects when it's stopped. If the connection fails or drops, it tries again every half a second. So keep listening, and accept a new connection after one ends. **Each new connection needs `settings` sent again**, and only one is expected at a time.

Turn on `TCP_NODELAY` on your side, as ephys-rs does on its own, to keep latency down. Latency can't be relied on, though: timing comes from `time`, not from when a message arrives.

## Messages

Each has a `type`, and the fields below. Fields can be in any order, and other fields are ignored. A line that isn't valid, or has an unknown `type`, is ignored and counted, and the first in each connection is logged.

The longest a message can be is 1 MiB.

### `time`

`pos`, `ttl` and `log` all need `time`: a number of seconds, as a double, from a clock of your choosing. A message without one is ignored.

- **Use one clock for all three**, so they can be compared with each other.
- **It has to be consistent for as long as the connection lasts**: always going forward at a steady rate, and never reset. Times counted from when the application started work well.
- It can start from any value. It should say when the thing happened, not when the message was sent.

ephys-rs keeps these times as they are, and only moves zero when writing files.

### `settings`

```json
{"type": "settings", "settings": {"window_min_x": 0, "window_max_x": 1000, "window_min_y": 0, "window_max_y": 1000, "pixels_per_metre": 500, "ttl_channels": 2, "task": "linear", "maze": {"arms": 8}}}
```

Each `settings` message is a **complete snapshot**, replacing the last. Send everything each time, not just what's changed. It has no `time`.

These are required, at the top level, and a message without all of them is refused, with an error logged:

| Key | Value |
| --- | --- |
| `window_min_x`, `window_max_x` | The range of `x`, with min below max |
| `window_min_y`, `window_max_y` | The range of `y`, with min below max |
| `pixels_per_metre` | How many of your made-up pixels there are in a metre: more than 0 |
| `ttl_channels` | How many TTL channels this task sends `ttl` messages on, from 0 (none) to 32 |

Anything else is yours to add, and is written to the headers of the `.pos` file, as `key value` on a line of its own. Nested objects have their keys joined with dots, so `"maze": {"arms": 8}` becomes `maze.arms 8`. Text is written as it is, and other values as JSON. Keys are written as they come, so a reader has to cope with any spaces in them, or with one matching another header.

Settings can't change during a recording, as the headers are already written. If different ones arrive, the recording keeps those it started with, an error is logged, and the latest apply when the recording stops. Sending the same settings again changes nothing.

### `pos`

```json
{"type": "pos", "time": 1520.26, "x": 412.5, "y": 87.0}
```

The animal's position, in made-up pixels, with `x` and `y` as numbers.

ephys-rs doesn't scale positions up, as the point is for the recording to look like a camera's to everything that reads it. It stores them as whole numbers, so the units need to be fine enough. A window about 1000 pixels across is ideal: a 2 m arena at 500 pixels per metre gives about 2 mm resolution. Windows wider than 1022 pixels are scaled down to fit. Positions outside the window are stored at its edge.

`y` goes down the screen in the Tracker View, as it does in video, so flip it if you'd like the view the other way up.

Send positions at a steady rate, and set the same rate on the node ("Position rate", 50 Hz by default). The rate there is what's recorded, and what the alarm on position data goes by.

### `ttl`

```json
{"type": "ttl", "time": 1520.25, "idx": 1, "value": 1}
```

TTL channel `idx` going up (`value` 1) or down (0), with both whole numbers. Channels are numbered from 1, as they are in the files and in ephys-rs, up to the `ttl_channels` in `settings`. A message without an `idx`, or with 0, is ignored and counted. One beyond `ttl_channels` isn't recorded, and an error is logged. Each message is recorded as a change, so only send them when a channel changes.

With `ttl_channels` 0, there's no `.pos.ttl` file.

In ephys-rs, a TTL Recording Control node can start recording when a channel goes up, and stop it when it goes down. To line the recording up with the electrophysiology, Post-Process Positions LTX uses one channel as the timing LED (see [below](#what-ends-up-in-the-recording)).

### `state`, from ephys-rs

```json
{"type": "state", "acquiring_for": 125.43, "recording_for": 12.31}
```

ephys-rs sends this once a second, and as soon as recording starts or stops:

| Key | Value |
| --- | --- |
| `acquiring_for` | Seconds since acquisition started (Play) |
| `recording_for` | Seconds since recording started, or `null` if it isn't recording |

They're by ephys-rs's own clock, not yours. Read these lines even if you ignore them. If one takes more than 0.2 s to send because they aren't being read, ephys-rs stops sending them until it connects again, and logs a warning.

### `log`

```json
{"type": "log", "time": 1520.25, "line": "trial 3 started"}
```

A line of text, for whatever's worth noting. Lines longer than the node's limit ("Log line limit", 256 bytes by default) are cut short, and line breaks become spaces.

## What ends up in the recording

For a recording called `name`:

| File | Has |
| --- | --- |
| `name.pos` | Positions, with the window, `pixels_per_metre` and your settings in its headers |
| `name.pos.ttl` | TTL changes, as `ttl_<idx> <seconds> <value>`, if `ttl_channels` isn't 0. Its headers are a camera's timing LED's: `timing_led_radius 0` says there's no real LED |
| `name.txt` | Log lines, as `<seconds> <line>`, with no headers |

All three count from the same zero: the first thing recorded, normally a position. So they can be compared directly.

They aren't on the acquisition board's clock, though. To line them up with the electrophysiology in post-processing, also send a real voltage TTL from the VR computer to one of the board's digital inputs, at the same moments as one channel's `ttl` messages. Post-Process Positions LTX then pairs the two: its **Positions' TTL line** is that channel's `idx`, and its **Board's TTL line** is the input.

## If messages come faster than they're taken

ephys-rs keeps up to 4,096 of each kind of message waiting, which is far more than steady use needs. Any more are lost, and counted in the node's window.
