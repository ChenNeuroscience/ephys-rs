# chen-neuro

The Chen lab's build of ephys-rs, and what this repository releases. It's the standard components, plus:

- **Quick actions** for the lab's rig, in the basic view: the standard LTX graph (an acquisition board and a FLIR camera, viewed and recorded by Write LTX), swapping between the camera and a VR computer, and swapping between real devices and synthetic data.
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

   It sends the fake camera's mouse as positions, with a TTL and a log line every few seconds. Use `--port` and `--fps` to change where it listens and how often it sends positions.

2. In the app's basic view, run the **Standard LTX graph** quick action, then **Use VR computer**. This adds the VR Computer and a Log Viewer, which connects to the script's default port, and takes the camera out of use.

3. Press Play. The Tracker View shows the mouse, and the Log Viewer its trials.

To see settings change, press Enter in the script's terminal. **Use camera** swaps back.

What a real VR computer has to send is in the [developer docs](../../docs/development.md#the-vr-computer).
