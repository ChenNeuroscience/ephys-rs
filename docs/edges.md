# Edges

What can flow along an edge is declared like node types are: each edge type is a small struct, gathered by
`edge_types!` at the root of the standard components (as `ephys_rs_standard::Edges` and so on, in
[`standard/src/lib.rs`](../standard/src/lib.rs)), with
the kinds the standard components are written against. The traits are in [`edge.rs`](../src/engine/edge.rs).

```rust
edge_types! {
    pub struct Edges;
    data_streams: pub enum DataStream, Kind, Buffer {
        Continuous(ephys::Continuous),
        // ...
    }
    values: pub enum PublishedValue, ValueKind {
        Thresholds(spikes::Thresholds),
        // ...
    }
    commands: pub enum Command, CommandKind {
        SetThresholds(spikes::Thresholds),
        ProcessFiles(run_command::Files),
        // ...
    }
}
```

That makes the `DataStream` and `PublishedValue` enums of values, the `Kind` and `ValueKind` enums that ports are declared
with, and the `Buffer` enum of the data streams' buffers; and the `Command` enum of what nodes can tell each other to
do, through value edges (or themselves, from their passes and threads), with `CommandKind` naming them (see
[Nodes](nodes.md#commands)). `Command` has one more variant than the program declares, the core's own
`ClearTransientState`.

## Data streams

A data edge carries a data stream from an output to an input. Its type is a small `Copy` description of the stream
(channels and sample rate, say), implementing `EdgeType` (its name, colour and description, shown on the graph and in
the legend, and a short summary: "64 ch · 30 kHz") and `DataStreamType`:

- `type Buffer`: how a pass's worth is held.
- `buffer(duration)`: an empty buffer with room for a pass. Buffers are made before running and reused for every pass,
  so it should allow for a pass that's a little longer than usual: [`Continuous`](../standard/src/ephys/mod.rs)
  has room for a quarter more, so a source can catch up after a hiccup.
- `bytes(buffer)`: its memory, for the dev tools.

Buffers are swapped between passes and threads (see the pools in [Architecture](architecture.md#threads-and-buffers)),
so a buffer made for a stream must be interchangeable with any other made for it: make them only with `buffer`.

The standard data streams are continuous data, digital events, spikes, video, positions, camera stats, spectra, alarm
states and log lines; each is defined beside the nodes that make it (`ephys`, `spikes`, `tracking`, `alarms`), and log
lines in `logs`.

## Values and commands

Value edges can be hidden, to clear up the graph, with the eye by Settings in the legend. Their ports stay, and they're
only hidden: nothing changes, and they come back while a new one's being dragged out. Hiding them deselects any that was
selected, so Delete can't remove something out of sight.

A value edge runs from a node's `ValInCmdOut` port to another node's `ValOutCmdIn` port of the same kind. Each port is
named for what flows through it, which way (as data ports are `DataStreamIn` and `DataStreamOut`):

- At the `ValOutCmdIn` end, the node **publishes a value** (`Node::val_out`), usually a copy of one of its settings,
  and takes **commands** through the edge (`Node::on_command`).
- At the `ValInCmdOut` end, the node reads the value (`UiContext::val_in`), and sends commands (`UiContext::cmd_out`):
  usually the one setting the value (`SetThresholds`, say), sometimes others (auto detect's `Detect`).
- A `ValIn` port is val-only: the node only reads the value (a writer recording the gains). The GUI calls it read-only.

A viewer setting a detector's thresholds reads them and sends `SetThresholds`; a writer recording the gains only reads
them. Value edges carry no data, so can point anywhere, upstream included; a `ValOutCmdIn` can have several edges into
it, a `ValInCmdOut` one out of it. A value's type (a list of thresholds, say) implements `EdgeType`; it needn't be
`Copy`. A command's type needn't implement anything but `Debug`. Settings stay a node's own: what it saves and loads, and
what's copied and pasted; what it publishes and what it's commanded to do are up to it.

A command sent through a port goes through the app's queue (see [`commands.rs`](../src/commands.rs)), addressed to the
port, not the node at the far end: the app follows the edge as it delivers it, on the GUI's thread. So a sender kept
for later (`CmdSender`) sends to where the edge points then.

Value edges are drawn dashed, with arrows both ways (dotted when read-only); the legend has one row for each, not a
row per kind of value. Disabled edges are broken in the
middle.
