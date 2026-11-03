# Nodes

A node type is a struct of its settings implementing [`Node`](../src/core/engine/node.rs), listed in `node_set!` in
[`main.rs`](../src/main.rs). Its settings are saved in the graph's file (with serde), copied and pasted, and undone;
two nodes of a type differ only in their settings. The simplest complete one is the
[Gain](../src/standard_components/ephys/gain.rs): one input, an output made in place, one setting.

![A node's window, and the graph](../tests/snapshots/graph_windows.png)

## Describing it

- `INFO`: its name, category (source, processing, visualisation or recording: its colour on the graph, and where
  it's offered) and description.
- `inputs()` and `outputs(inputs)`: its data ports. An output says what data stream it produces, given what's on the
  inputs (or `None` while that can't be known), so a graph's streams are worked out before anything runs. A node runs
  only if all its outputs are known.
- `warnings(inputs)`: what's wrong (an input not connected, say), shown on the graph and in its window.
- `settings()` and `set_setting`: the settings other nodes can edit through settings edges (see [Edges](edges.md)).
- `controls()`: the other side, settings of other nodes this one can see and edit (or only read, `read_only`), in its
  `Context::controls`.

## Running

`process(pass, state)` is called once a pass, on a worker thread, with the pass's buffers (see
[`Pass`](../src/core/engine/run.rs)): `inputs` to read, `outputs` to fill (already sized: nothing should be allocated
per pass), the settings its `controls` see, whether it's recording, and the exchange. `self` is the snapshot's
settings: the latest, as of the pass.

- **State.** `type State` is what it keeps between passes, made by `initial_state(inputs)`, and `()` for none. It
  implements `PassState`, saying why it's needed (`WHY`, shown in the dev tools at the sync point it makes). State
  that's only working memory (nothing carried over: `CARRIED_OVER = false`) is each worker's own, and makes no sync
  point. State is fresh each epoch.
- **In place.** `in_place(output)` names an input whose buffer the output can be made in, when nothing else needs that
  data after it. Then the input's `None` in the pass, and the output already holds its data.
- **Taking.** `takes(input)`: the node would take the input's buffer, to swap for one of its own, rather than copy it
  out (a writer, handing it to its thread). When it can, `Pass::take_input` gives it the buffer; otherwise it reads it.
- **Needs.** `needs(input)`: whether it needs an input's data now, given its settings. If not, the plan doesn't pass it,
  and doesn't run a node whose outputs nothing needs (the LFP viewer's spectrum, switched off, stops the
  [Spectrum](../src/standard_components/ephys/spectrum.rs) node). Changing it changes the plan, so it's for what's
  switched on and off.
- **Copying out.** `copies()`: it only copies its inputs out, to its window or a thread, so it can run early.

## Its window

Double-clicking a node opens its window: `view_ui` if it has a view (`HAS_VIEW`), otherwise `settings_ui`, with its
name, warnings and ports. Both get a [`Context`](../src/core/engine/node.rs):

- `inputs`: the streams on its inputs; `controls`: the values of the settings its controls see, to edit.
- `display`: the latest its passes left in the exchange (send it from `process` with
  [`Latest`](../src/core/exchange.rs)).
- `running` and `locked` (while recording, settings changes are undone, so show them disabled).
- `advanced()`: whether the app's in advanced mode. What a view shows in basic mode is up to it, but basic is meant to
  be simpler; options for it go in its settings menu.
- `node_toolbar` / `node_toolbar_with`: the buttons every node has (copy and paste settings; in advanced mode, its
  settings menu, enabling and deleting it), for a view to put where it likes, with options of its own added to the
  settings menu.
- `trigger`, `can_trigger`, `trigger_progress`, `open_control_target`: see below.

The [Spike Viewer](../src/standard_components/spikes/spike_viewer.rs) and [LFP Viewer](../src/standard_components/ephys/lfp_viewer.rs)
are full examples.

## Triggers and proposals

A setting can offer **triggers** (`triggers(setting)`), as well as be set: "auto detect", say. A control connected to
it fires one (`Context::trigger(Some(control), name)`), as can the node's own window (`None`); the node's next pass
takes it (`Pass::triggered`), reports progress (`Pass::trigger_progress`), and finishes (`Pass::trigger_done`).
Meanwhile it can be stopped (`Context::cancel_trigger`): the passes then see `Pass::trigger_cancelled`, and abandon it. Since
passes can't change the graph, what it finds it proposes (`Pass::propose(setting, value)`), and the GUI applies it as
an edit. The [EEG Gain](../src/standard_components/ephys/eeg_gain.rs), [Spike Gain](../src/standard_components/spikes/spike_gain.rs) and
[Spike Detector](../src/standard_components/spikes/spike_detector.rs) work this way, triggered from the Spike Viewer.

## Threads of its own

A node can run threads (started in `initial_state`, stopped when its state's dropped). It lists them in `threads()`,
with which way data goes and why it's a thread, draws what each is up to in `thread_ui`, and reports any pool of
buffers it swaps with the passes in `thread_pool`, all for the dev tools. A source's thread fills buffers shaped like
the node's outputs from a [`Pool`](../src/standard_components/pool.rs), which `process` swaps into the pass; see the
[board's capture](../src/standard_components/ephys/board/capture.rs). A writer's passes swap their data into chunks
from a pool, for its thread to write; see [`recording`](../src/standard_components/recording/mod.rs).

## Recording

`prepare_to_record` is called on a thread of its own before recording starts (a writer opening its files), and what
it returns comes back to the first pass that records (`Pass::take_prepared`). Passes see `Pass::recording`.

## Testing

A node's `process` can be run on its own: make buffers from a stream (`stream.buffer(PASS_DURATION)`), and a pass
with `Pass::new`. Most node files have tests doing just that; for triggers, fire one on the pass's exchange (see the
Spike Gain's tests). Views are tested headless with `egui_kittest`, against images in `tests/snapshots` (see
[Development](development.md)).
