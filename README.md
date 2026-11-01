# ephys-rs
electrophysiology software


## Super high level vision

- Adobe AfterEfects inspired graph interface with lots of window panes.
- In code, each node type is fairly self contained module, but the whole program is a single compliation unit (in the near term at least). This means we can avoid designing an ultra generic API, and can instead support a metaprogramming solution of some sort. 
- Use the same ui machinery as the open-ephys-plugin-camera, i think.
- Need a basic buffer and threading model that supports high throughput.
- Each node type exposes a certain number of inputs and outputs, each with some kind of requirements info. And then when you click on the connectors it highlights mathcing nodes already in the graph and also offers a list of supported types that could go there to create a new node.
- Visualiser nodes, recorder nodes, settings nodes, synchronisation nodes, source nodes ... they all work pretty much the same way.
- Support for raw recording and for spikes-only recording.


We will have a pool of data processing threads, each thread will execute a full pass over the graph.

The input to that pass will be a reference to a frozen graph version (continaing all configuration options), plus a filled buffer of data from each source in the graph, plus pre-allocated buffers for processing needs in the graph. It then executes the processing over the whole of the provided graph. The number of threads will be configurable with some feedback to the user on how many are needed and if passes are taking too long.  
Unfortunately this is not enough though because many nodes will need output from their previous pass (e.g. band pass filters, spike detection). To support this, nodes can output data to chain to their next execution (a kind of reduction). We will use a dedicated flag (on its own cache line) for
each required synchronisation point, and a reference to that and the associated previous output buffer will be provided at the start of the pass (since the previous pass must have already allocated it before starting). Then we'll use somehting like this to wait on readiness. Note that as long as each pass stays ahead of the previous pass there will be very miniaml blocking, and even if it gets ahead it wont be that expensive to spin read a single cache line until the other thread grabs it to write with.

```
// Producer
ready.store(sequence, Ordering::Release);

// Consumer
while ready.load(Ordering::Acquire) != sequence {
    std::hint::spin_loop();
}
```

One more complex idea for writer threads that avoids synchronisation is this, but i don't think it's worth it given we are going to need to force synchronisation in other cases:

 Writer nodes will operate on this same thread and must write to their own fragment files during recording and then finalise by combining the recorded files into the right order. (we should offer some support for finalising afer a crash too, which ought to be a similar thing). This avoids true statefulness for writing (it is still stateful within the threads, but not across). But if you really do need something stateful that's a bit more painful as you would need some proper sychronisation mechanism where one thread can wait for the previous pass to complete the given thing..but i'm not sure we need 



how can we have a downstream node control settings upstream? maybe you can just allow that as a special kind of connection that's a settings connection not a data connection and it doesn't actually run in the graph like data.

## Plan

- Do the basic GUI, not worrying at all about the actual data processing to begin with.
- Create simmulated voltage and camera input nodes, band pass filters, gains, spikes.
- Switch in real camera, based on the open-ephys-plugin-camera.
- Switch in real acquisition.
- Record data.
- ...

## Downloads and development

Each push to `main` is tested and built on Windows and macOS by GitHub Actions, and published as a [release](https://github.com/ChenNeuroscience/ephys-rs/releases) named after the commit. On macOS the app isn't notarised, so the first time, right-click it and choose Open.

```bash
cargo run --release              # starts with an example graph
cargo run --release -- rig.json  # opens a graph file
cargo test                       # includes GUI tests, rendered headless against tests/snapshots
UPDATE_SNAPSHOTS=1 cargo test    # accept changes to how the GUI looks
```

The icon is `assets/icon.svg`; `scripts/export_icon.js` exports the PNGs from it.

For a camera to track without real hardware, `python3 tools/fake_camera.py` streams video of a mouse with two head LEDs in an arena; set a Camera node's source to the fake camera to use it.
