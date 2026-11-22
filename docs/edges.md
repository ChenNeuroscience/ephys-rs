# Edges

Edge types (what can flow along an edge) are declared much like node types. Each edge type is a small struct. Each
build lists the edge types its nodes need, whether standard or its own, with `edge_types!` (see chen-neuro's
[components](../examples/chen-neuro/src/components/mod.rs) for an example). The traits are in
[`edge.rs`](../src/engine/edge.rs).

```rust
edge_types! {
    pub struct Edges;
    data_streams: pub enum DataStream, DataStreamKind, Buffer {
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

This generates:

- the `DataStream` and `PublishedValue` enums, and the `DataStreamKind` and `ValueKind` enums naming their variants,
  which the graph matches ports by;
- the `Buffer` enum of the data streams' buffers;
- the `Command` enum of the commands nodes can send each other through ValCmd edges (or send themselves, from their
  passes and threads), and `CommandKind` naming them (see [Nodes](nodes.md#commands)). Each command is a type of its
  own, such as `SetThresholds(pub Thresholds)`.

For each type listed, `edge_types!` also implements `HasDataStream`, `HasValue` or `HasCommand` on the build's
`Edges`. These traits convert between the type and the build's enum, so nodes can declare their ports (see
[Nodes](nodes.md#describing-it)) and work with what's on them using the types themselves, never naming a build's
enums.

Each node is written for any build's edges that include the types its ports carry: `impl<E: PortEdges> Node<E> for
MyNode`, where `ports!` generates `PortEdges` from the node's ports. So a build lists only the types its nodes need.
If a build leaves out a type that one of its nodes needs, the build doesn't compile, and the error names the missing
type ("the program's edges have no `Spikes` data stream"). A list can be empty: a build whose nodes send no commands
lists none.

## Data streams

A data edge carries a data stream from an output to an input. Its type is a small `Copy` description of the stream
(channels and sample rate, say), implementing `EdgeType` (its name, colour and description, shown on the graph and in
the legend, and a short summary: "64 ch · 30 kHz") and `DataStreamType`:

- `type Buffer`: how a pass's worth is held.
- `buffer(duration)`: an empty buffer with room for a pass. Buffers are made before running and reused for every pass,
  so it should allow for a pass that's a little longer than usual: [`Continuous`](../standard/src/ephys/mod.rs)
  has room for a quarter more, so a source can catch up after a hiccup.
- `bytes(buffer)`: its memory, for the dev tools.

Nodes declare their data ports with these types, such as `continuous: DataStreamIn<Continuous>` (see
[Nodes](nodes.md#describing-it)), and work with the streams and buffers on those ports directly as these types.

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
`Copy`. A command's type only needs to implement `Debug` (and be `Send`), and is usually a struct holding the data the
command carries, such as `SetGains(pub Gains)`. Each `ValOutCmdIn` or `ValInCmdOut` port is declared with the
commands it accepts or sends, as a tuple of their types: `ValOutCmdIn<Gains, (SetGains, Detect)>`. A window can't
send a command that its port doesn't list, and a node only receives the commands its ports accept. Settings stay a node's own: what it saves and loads, and
what's copied and pasted; what values it gives its `ValOutCmdIn` ports and what it does with commands are up to it.

Each end of an edge is a port's address (`PortAddress`): its node, role, and number among the node's ports of that
role. Saved graphs identify each end by the port's id instead of its number (`SavedPort`:
`{"node": 1, "role": "DataStreamOut", "port": "headstages"}`). The id is the port's field name in the node's
`ports!`, so a node's ports can be reordered without breaking files, but renaming a port needs a config patch. When a
file is loaded, an edge with an end naming a port the node doesn't have is dropped, with a warning. Whether an edge
carries data or a value (`Edge::carries_data_stream`) follows from the roles of its ends.

A node numbers the ports it reads values through (`ValInCmdOut` and `ValIn`) together, in declaration order. The node
reads values and sends commands through handles to those ports, such as `UiContext::val_in(ValIn::gains)` and
`UiContext::cmd_out(ValInCmdOut::gains, SetGains(gains))`. Each handle holds the port's number (see
`val_in_address`).

A command sent through a port goes through the app's queue (see [`commands.rs`](../src/commands.rs)), addressed to the
port, not the node at the far end: the app follows the edge as it delivers it, on the GUI's thread. So a sender kept
for later (`CmdSender`) sends to where the edge points then.

ValCmd edges are drawn dashed, with arrows both ways (dotted when read-only); the legend has one row for each, not a
row per kind of value. Disabled edges are broken in the
middle.
