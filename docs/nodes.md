# Nodes

A node type is a struct of its settings implementing [`Node`](../src/core/engine/node.rs), listed in `node_set!` in
a program's components (the [example's](../example/src/components/mod.rs), say). Its settings are saved in the graph's file (with serde), copied and pasted, and undone;
two nodes of a type differ only in their settings. A simple complete one is the
[Channel Selector](../src/standard_components/ephys/channel_selector.rs): one input, one output, one setting (which
another node can edit).

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
- **Taking.** `takes(input)`: the node takes the input's buffer, to do as it likes with (swap it for one of its own,
  say, to hand to its thread or window). Nodes don't copy their inputs; the plan decides when a copy's needed.
  `Pass::take_input` always gives the node a buffer: the input's own, if no one edits that data in place and every
  other reader can run first, otherwise a copy the plan makes as soon as the data's made (a copy point, in the dev
  tools). A node that only reads an input doesn't take it.
- **Needs.** `needs(input)`: whether it needs an input's data now, given its settings. If not, the plan doesn't pass it,
  and doesn't run a node whose outputs nothing needs (the LFP viewer's spectrum, switched off, stops the
  [Spectrum](../src/standard_components/ephys/spectrum.rs) node). Changing it changes the plan, so it's for what's
  switched on and off.

## Its window

Double-clicking a node opens its window: `view_ui` if it has a view (`HAS_VIEW`), otherwise `settings_ui`, with its
name, warnings and ports. Both get a [`Context`](../src/core/engine/node.rs):

- `inputs`: the streams on its inputs; `controls`: the values of the settings its controls see, to edit.
- `shared::<T>()`: what its passes share with it in the run's [exchange](../src/core/exchange.rs), while running (see
  below).
- `running` and `locked` (while recording, settings changes are undone, so show them disabled).
- `advanced()`: whether the app's in advanced mode. What a view shows in basic mode is up to it, but basic is meant to
  be simpler; options for it go in its settings menu.
- `node_toolbar` / `node_toolbar_with`: the buttons every node has (copy and paste settings; in advanced mode, its
  settings menu, enabling and deleting it), for a view to put where it likes, with options of its own added to the
  settings menu.
- `trigger`, `can_trigger`, `trigger_progress`, `open_control_target`: see below.

The [Spike Viewer](../src/standard_components/spikes/spike_viewer.rs) and [LFP Viewer](../src/standard_components/ephys/lfp_viewer.rs)
are full examples.

## Sharing with its window

The engine doesn't hand anything to a node's window itself. Each run has an [`Exchange`](../src/core/exchange.rs),
where a node keeps whatever its passes and its window (or the dev tools: `ThreadContext::shared`) both reach, of any
type implementing `Shared`: `Pass::shared(make)` finds or makes it (keep the `Arc` in the node's state, rather than
finding it every pass), and `Context::shared` finds it. A `Shared` can report a pool of buffers (`Shared::pool`),
which the dev tools show on the GUI.

The usual thing to share is a [`Pool`](../src/standard_components/pool.rs) of buffers: the same cross-thread pool
sources and writers use with their threads. A `SharedPool<T>` in the node's state finds (or makes) the node's pool
in the exchange, and the window finds it as `Pool<T>`. For something to show, only the newest matters:

```rust
// In the state: the node's pool (its buffers made by T::default, or by `SharedPool::new`: to size, say).
shown: SharedPool<TracesDisplay>,

// In process: fill a buffer (an earlier send's, to overwrite), and queue it as of the pass.
state.shown.send_newest(pass, |display| display.fill_from(...));

// In view_ui: the newest queued, held for as long as the view likes.
let pool = context.shared::<Pool<TracesDisplay>>();
let Some(display) = pool.as_deref().and_then(Pool::newest) else { return };
```

Neither side waits: passes don't skip a send while the window's reading, and if the window's closed, passes reuse the
oldest buffer queued. A node that takes its input can swap it straight in (`SharedPool::pool`, then `Pool::reuse` and
`Pool::send_as_of`), so nothing's copied: the [Video Viewer](../src/standard_components/tracking/video_viewer.rs)
does this with frames.

## Triggers and proposals

A setting can offer **triggers** (`triggers(setting)`), as well as be set: "auto detect", say. A control connected to
it fires one (`Context::trigger(Some(control), name)`), as can the node's own window (`None`); the node's next pass
takes it (`Pass::triggered`), reports progress (`Pass::trigger_progress`), and finishes (`Pass::trigger_done`).
Meanwhile it can be stopped (`Context::cancel_trigger`): the passes then see `Pass::trigger_cancelled`, and abandon it. Since
passes can't change the graph, what it finds it proposes (`Pass::propose(setting, value)`), and the GUI applies it as
an edit. The [Continuous Gain](../src/standard_components/ephys/continuous_gain.rs), [Spike Gain](../src/standard_components/spikes/spike_gain.rs) and
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
