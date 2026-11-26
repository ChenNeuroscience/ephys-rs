# Quick actions

Quick actions are ready-made changes to the graph, offered in basic mode to people who'd rather not build it node by
node: chen-neuro's "Standard LTX graph", "Use VR computer", "Use synthetic data".

![Basic mode, with quick actions](../tests/snapshots/basic_after_quick_actions.png)

A [`QuickAction`](../src/quick_action.rs) is data, not code, so people can make, change and share their own. A
build ships some, as files listed in `Components::QUICK_ACTIONS` (chen-neuro's are in
[`quick_actions/`](../examples/chen-neuro/quick_actions), each `include_str!`'d by
[`quick_actions.rs`](../examples/chen-neuro/src/components/quick_actions.rs)). Every quick action is a file in a folder
beside the app's settings (`quick_actions/` in eframe's storage directory), listed in Settings › Quick actions with its
description and when it was last changed. At start, the build's quick actions are copied into the folder, each as
it is, to a file named for it, unless the folder has had it before (listed in `.built-in`) or a file of that name is
there already. From then on, they're files like any other: changed, or deleted (and not given again), as people like,
and an update to the build doesn't change the ones already given.

## What a quick action is

A quick action is a small graph whose nodes are each a **role**, with a rule (`Rule`) for how it's found in the graph
it's done on, and what's done to it:

- **Matched**: by the node's type, and a regular expression its label matches (`label_pattern`, such as `(?i)low|lfp`;
  without one, `.*`, any label; an empty one matches nothing). Each role is one node, and each node one role's at most: a role with only one candidate
  is paired with it first (and that node struck off the rest's candidates); when none has just one, the left-most
  role takes its left-most candidate (of two equally far left, the higher); and so on (`QuickAction::find`).
- **When found**: `settings`, the settings (by name) applied as the quick action has them, and `enabled`, whether the
  node's enabled or disabled (or, without it, left as it is). A node created is always enabled.
- **When not found** (`mode`): `ensure` creates the node, with all its settings (the default); `if_present` doesn't
  create it, and the quick action is still offered, leaving out its edges and window; `require` means the quick
  action isn't offered. Or the role's ignored (`ignore`): never matched, and its edges left alone, so in a quick
  action made from a whole graph the parts that don't matter to it can stay, to show where the parts that do are.

Each edge is between roles, by port id, with what's done to it in each case (`EdgeRule`): if the nodes found are
connected that way already, `enabled` (true or false; or, without it, left as it is); else, if the end that takes just one edge (a data stream's
input, or the reading end of a value) is free, `connect` them (the default) or not; else (that end connected to
something else), `replace` that, or not. A quick action never disconnects an edge; an edge it connects is enabled.
An edge that does nothing in any case just shows what surrounds the rest.
Nodes are never removed: a quick action disables them, which
keeps their settings for when they're wanted again.

A role's `window` says what's done with the node's window: if it's open, it's closed (`"close": true`) or left open;
if it isn't, it's opened (`"open": placement`) or left closed. A window that's been open before opens where it was;
opened for the first time, it goes as the placement says: `"default"` (where the app puts a new window: see
`Layout::place`), `"largest"` (in front, as a tab in the largest group), `{"behind": role}` (as a tab behind that
role's window) or `{"beside": {"of": role, "side": "right", "share": 0.5}}` (a group of its own on that side of that
role's window's group, taking that share of the space). Placed relative to a window that isn't open, it goes where
the app puts a new window. `panes.arrangement` arranges the panes (see [the layout](#the-layout)), each window by
role. `base_directory_type` chooses the [base directory](base-directories.md) (not while recording), and
`arrange_if_empty` lays out a graph built from nothing afresh.

A quick action is offered while its required roles are found and doing it would change the graph (see
`QuickAction::run`). So one that's done isn't offered until something changes, and closing a window it opened doesn't
bring it back. One that can't change the graph (that only opens and closes windows) is offered while it would change
which windows are open. Doing one is undone in one step.

The roles are found in the order they're listed. A role added goes where it is in the quick action relative to a node
it's connected to that's in the graph, or, with none, below the graph; then down, clear of what's there.

chen-neuro's quick actions show each of these: the standard LTX graph ensures everything, telling the spikes' and the
LFP's band-pass filters (Band-pass High and Band-pass Low) apart by their labels; "Use VR computer" and "Use camera"
disable one source, enable the other, replace the edges into the tracker view, Write LTX and the Alarm Processor, and disable (or enable again) the
VR computer's other edges;
"Use synthetic data" and "Use real devices" set `source` on the board and the camera.

## The file

A quick action's file (`name.quick-action.json`) is shaped as a project, with each node's rule beside it (`"quick"`)
and each edge's rule on it (`"enabled"`, `"connect"`, `"replace"`), so the app's [config patches](development.md#config-patches) bring an old one up to date as they would
a project:

```json
{
  "ephys-rs quick action": { "core": "0.310.0", "std": "0.310.0", "chen-neuro": "0.1159.0" },
  "name": "Use synthetic data",
  "description": "The fake acquisition board and camera, instead of real devices",
  "graph": {
    "next_id": 3,
    "nodes": {
      "1": {
        "label": "Acquisition Board", "pos": [0, 0],
        "node": { "type": "AcquisitionBoard", "settings": { "...": "..." } },
        "quick": {
          "mode": "if_present", "settings": ["source"]
        }
      }
    },
    "edges": []
  },
}
```

A quick action with a node type the build doesn't have can't be read, and Settings says so.

## Making and changing them

In Settings › Quick actions, **New from the graph** (or from the nodes selected) makes one from the graph: each node a
role, ensured, setting nothing on what's found; the edges between them, ensured; their open windows, opened. Each
quick action has buttons to edit it, copy it to the clipboard, and delete its file; the folder icon opens the folder.

The editor shows the quick action's nodes and edges as a graph, on a canvas like the app's, most of the window's
size:

![The quick action editor](../tests/snapshots/quick_action_editor_node.png)

- Under each node, what the quick action does with it, in the inspector's words ("If found: disable", "Else:
  create the node", or without an "If found", "If not found: create the node"; "Window: open it if not"), coloured for what happens if it isn't found. Edges that may
  replace another are drawn in the warning colour, and those that do nothing faded.
- The nodes and edges are those the quick action was made with: none can be added or removed in the editor (there's
  no undo there, nor settings to edit), so to change which are in it, make a new one from the graph, or untick the
  parts that don't matter. **Arrange** lays them out afresh.
- Beside the canvas, the inspector changes what's selected, under a bar naming it (for a node, in its colour, as its
  window's tab is), in sections drawn as a tree; each pair that's one choice (if found, else if not; if the window's
  open, else if not) is drawn as branches of one line. For a node: whether it's used (the box at the right of its
  bar; unticked, it's ignored, and drawn faded); how it's matched (its type, and a label pattern, `.*` by default,
  with a cheat sheet); if found, whether to enable or disable it, and whether to apply settings, and which; else,
  whether to do nothing, create it, or not offer the quick action; and its window, if open (do nothing, or close
  it), else (do nothing, or open it, and where, the first time). "Do nothing" is the first option of every choice
  that has one. For a connection: in a box with a checkbox to use it (unticked,
  it's ignored), its two nodes, each with its port on its right edge, joined by the wire as the canvas draws it; and
  what's done in each case, as branches (if connected already, enable or disable; else, if the input's free, connect;
  else, if it's occupied, connect, replacing that; either way, the source's other connections are left as
  they are). With nothing selected, the quick action's name, description, arrangement and base
  directory. An ⓘ explains the few things that aren't plain: which node is used when several match, the
  label pattern's syntax, connecting "if free", and where a window opens the first time.
- Ignored nodes and edges are drawn faded, with no note.
- **Take the settings of** sets a node's settings from the node it's found as, so one way to change a quick action is
  to do it, change the graph by hand, and take the settings back.

The editor's canvas is the app's (`gui::canvas::Canvas`), drawing the quick action's own graph (`QuickAction::graph`,
put back with `set_graph`). What's different is given by `canvas::Extras`: the badges, the edges' colours, and the
editor's own menus, without annotations, windows or pasting. The app's canvas uses `()`, which adds nothing.

**Copy** puts a quick action on the clipboard, and pasting one (in Settings, or anywhere settings can be pasted) opens
it in the editor, to save. With the files, that's how they're shared.

## The layout

The [`Layout`](../src/layout.rs) of panes (home, the dev tools, the annotation pane and node windows: each a tab
in a group, the groups side by side or one above another) has the helpers the app itself uses to place panes, which
quick actions use on a copy of it:

- `open(id, from)` and `close(id)`: a node's window, opened where it was last, or (the first time) placed by `place`.
- `place(pane, from)`: a step towards the **default layout** (`Arrangement::default_layout`: a column a third of the
  width on the left, the full height; on the right, one group above another, two thirds and a third of the height).
  With one group showing, a new column on its right; with two, the second split, the new one below; after that, a tab
  in the largest group other than the one it was opened from (`from`, if it was opened from a pane).
- `largest_group(except)`, `beside(group)` (the group on its right, or failing that its left), `group_of(pane)`,
  `add_tab(group, pane)`, `split(target, pane, side, share)`: for placing panes otherwise. Sizes are worked out from
  the layout itself (`rects`), so panes placed one after another are placed as they'll be.
- `arrange(arrangement)`: replaces the layout with an `Arrangement` of rows, columns and tabs, the panes it doesn't
  place gathered where it says (`Tab::Others`). The standard LTX graph arranges its viewers in the default layout.
  `arrangement()` gives the layout as it is, as an `Arrangement`, for a quick action to keep.

The app places panes it opens itself the same way: a node's window with `place` (from the pane it was opened from:
the graph, the basic view, or another window); the dev tools as a tab in the largest group; the annotation pane in
the group beside home, or failing that a new column on its right.
