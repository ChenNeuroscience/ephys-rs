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
    settings: pub enum Setting, SettingKind {
        Thresholds(spikes::Thresholds),
        // ...
    }
    triggers: pub enum Trigger, TriggerKind {
        AutoDetect(auto_detect::AutoDetect),
        // ...
    }
}
```

That makes the `DataStream` and `Setting` enums of values, the `Kind` and `SettingKind` enums that ports are declared
with, and the `Buffer` enum of the data streams' buffers; and the `Trigger` enum of what can be sent along settings
edges (or by a node itself), with `TriggerKind` naming them (see [Nodes](nodes.md#triggers)).

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

## Settings

Settings edges can be hidden, to clear up the graph, with the eye by Settings in the legend. Their ports stay, and they're
only hidden: nothing changes, and they come back while a new one's being dragged out. Hiding them deselects any that was
selected, so Delete can't remove something out of sight.

A settings edge runs from a node's **control** to another node's **setting**, letting it edit (or, if the control's
read-only, just see) the setting: a viewer setting a detector's thresholds, a writer reading the gains to record them.
They carry no data, so can point anywhere, upstream included; a setting can have several controls, a control one
setting. A setting's type is its value (a list of thresholds, say), implementing `EdgeType`; it needn't be `Copy`.

When a node's window edits a control's value (`UiContext::controls`), the graph sets it on the setting's node
(`Node::set_setting`), as an edit. A setting can also offer triggers (see [Nodes](nodes.md#triggers)).

Settings edges are drawn dashed, with arrows both ways (dotted when read-only); the legend has one row for each, not a
row per setting. Disabled edges are broken in the
middle.
