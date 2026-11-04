# Quick actions

Quick actions are ready-made changes to the graph, offered in basic mode to people who'd rather not build it node by
node: "Standard LTX graph v0.5", "Disable position tracking", "Use synthetic data".

![Basic mode, with quick actions](../tests/snapshots/basic_after_quick_actions.png)

A [`QuickAction`](../src/core/quick_action.rs) is a name, a description, and a pure function from a
`QuickState` (the graph, with all its settings, and the [layout](#the-layout) of panes) to a `QuickChange`, or nothing
if it doesn't apply (when it's already been done, say), when it isn't offered. Being a pure function, it's undone in
one step, and easy to test.

A `QuickChange` is:

- `graph`: the graph to change to.
- `layout`: the layout to change to: a changed copy of the one it was given.
- `arrange`: whether to lay the graph out afresh, as after building it from nothing.
- `base_directory`: the [base directory](base-directories.md) to use, by its type's name, if it says (not while
  recording), as the standard LTX graph chooses subject, date and trial.
- `summary`: what it did, in a line, which is logged. By convention it says what was added, connected, or changed.

A program supplies its own through `Components`. chen-neuro's are in
[`quick_actions.rs`](../examples/chen-neuro/src/components/quick_actions.rs), listed in `QUICK_ACTIONS`. Its `Change` helper
reuses what the graph already has (`find`, `upsert`), adds what's missing beside what it follows (`add`), connects
ports that are free (`link`), and sums up what it did (`finish`), so an action applies to a graph that's half there,
and isn't offered once it's all there. `finish` opens the windows it's given in the layout (those not open already),
where they'd go.

## The layout

The [`Layout`](../src/core/layout.rs) of panes (home, the dev tools, the annotation pane and node windows: each a tab
in a group, the groups side by side or one above another) has the helpers the app itself uses to place panes, for
actions to use on their copy:

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

The app places panes it opens itself the same way: a node's window with `place` (from the pane it was opened from:
the graph, the basic view, or another window); the dev tools as a tab in the largest group; the annotation pane in
the group beside home, or failing that a new column on its right.
