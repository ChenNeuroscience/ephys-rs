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
        SetThresholds(spikes::SetThresholds),
        ProcessFiles(run_command::ProcessFiles),
        // ...
    }
}
```

That makes the `DataStream` and `PublishedValue` enums of values, the `Kind` and `ValueKind` enums that ports are declared
with, and the `Buffer` enum of the data streams' buffers; and the `Command` enum of what nodes can tell each other to
do, through ValCmd edges (or themselves, from their passes and threads), with `CommandKind` naming them (see
[Nodes](nodes.md#commands)). Each command is a type of its own (`SetThresholds(pub Thresholds)`). For each type listed,
`edge_types!` also implements `StreamOf`, `ValueOf` or `CommandOf`, which go between the type and the program's enum,
so nodes declare their ports (see [Nodes](nodes.md#describing-it)) and work with what's on them as their own types,
never naming the program's enums.

## Data streams

A data edge carries a data stream from an output to an input. Its type is a small `Copy` description of the stream
(channels and sample rate, say), implementing `EdgeType` (its name, colour and description, shown on the graph and in
the legend, and a short summary: "64 ch · 30 kHz") and `DataStreamType`:

- `type Buffer`: how a pass's worth is held.
- `buffer(duration)`: an empty buffer with room for a pass. Buffers are made before running and reused for every pass,
  so it should allow for a pass that's a little longer than usual: [`Continuous`](../standard/src/ephys/mod.rs)
  has room for a quarter more, so a source can catch up after a hiccup.
- `bytes(buffer)`: its memory, for the dev tools.

Nodes declare their data ports by these types (`continuous: DataStreamIn<Continuous>`: see
[Nodes](nodes.md#describing-it)), and work with what's on them as these types and their buffers.

Buffers are swapped between passes and threads (see the pools in [Architecture](architecture.md#threads-and-buffers)),
so a buffer made for a stream must be interchangeable with any other made for it: make them only with `buffer`.

The standard data streams are continuous data, digital events, spikes, video, positions, camera stats, spectra, alarm
states and log lines; each is defined beside the nodes that make it (`ephys`, `spikes`, `tracking`, `alarms`), and log
lines in `logs`.

## Values and commands

ValCmd edges can be hidden, to clear up the graph, with the eye by Settings in the legend. Their ports stay, and they're
only hidden: nothing changes, and they come back while a new one's being dragged out. Hiding them deselects any that was
selected, so Delete can't remove something out of sight.

A ValCmd edge runs from a node's `ValInCmdOut` port to another node's `ValOutCmdIn` port of the same kind. Each port is
named for what flows through it, which way (as data ports are `DataStreamIn` and `DataStreamOut`):

- At the `ValOutCmdIn` end, the node **publishes a value** (`Node::val_out`), usually a copy of one of its settings,
  and takes **commands** through the edge (`Node::on_command`).
- At the `ValInCmdOut` end, the node reads the value (`UiContext::val_in`), and sends commands (`UiContext::cmd_out`):
  usually the one setting the value (`SetThresholds`, say), sometimes others (auto detect's `Detect`).
- A `ValIn` port is val-only: the node only reads the value (a writer recording the gains). The GUI calls it read-only.

A viewer setting a detector's thresholds reads them and sends `SetThresholds`; a writer recording the gains only reads
them. ValCmd edges carry no data, so can point anywhere, upstream included; a `ValOutCmdIn` can have several edges into
it, a `ValInCmdOut` one out of it. A value's type (a list of thresholds, say) implements `EdgeType`; it needn't be
`Copy`. A command's type needn't implement anything but `Debug`, and is usually a struct holding what it carries
(`SetGains(pub Gains)`). A `ValOutCmdIn` or `ValInCmdOut` port's declared with the commands it takes or sends, as a
tuple of their types (`ValOutCmdIn<Gains, (SetGains, Detect)>`): a window can't send a command its port doesn't
list, and a node is handed only the commands its ports take. Settings stay a node's own: what it saves and loads, and
what's copied and pasted; what values it gives its `ValOutCmdIn` ports and what it does with commands are up to it.

Each end of an edge is a port's address (`PortAddress`: its node, role and index among the node's ports of that role).
Saved, each end names its port by its id instead (`SavedPort`: `{"node": 1, "role": "DataStreamOut", "port":
"headstages"}`), the field's name in the node's `ports!`, so a node's ports can be reordered without breaking files,
but renaming one needs a config patch; an end naming a port its node hasn't got is dropped, with a warning, as the file's
read. Whether an edge carries data or a value (`Edge::carries_data_stream`) follows from the roles. A
node itself numbers the ports it reads values through (`ValInCmdOut` and `ValIn`) together, in the order it declares
them, and reads and commands through handles to them (`UiContext::val_in(ValIn::gains)`,
`UiContext::cmd_out(ValInCmdOut::gains, SetGains(gains))`), each holding that number (see `val_in_address`).

A command sent through a port goes through the app's queue (see [`commands.rs`](../src/commands.rs)), addressed to the
port, not the node at the far end: the app follows the edge as it delivers it, on the GUI's thread. So a sender kept
for later (`CmdSender`) sends to where the edge points then.

ValCmd edges are drawn dashed, with arrows both ways (dotted when read-only); the legend has one row for each, not a
row per kind of value. Disabled edges are broken in the
middle.
