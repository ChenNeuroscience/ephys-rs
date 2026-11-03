# Developer docs

How ephys-rs works under the hood, and how to extend it. Start with the architecture, then read about whichever part
you're working on.

- [Architecture](architecture.md): the graph and its snapshots, plans, passes and epochs; threads, buffers and pools;
  how the GUI and the passes talk; core and standard components.
- [Nodes](nodes.md): writing a node type, from settings and ports to state, views, threads and triggers.
- [Edges](edges.md): data streams and settings, and how buffers are made.
- [Annotations](annotations.md): things on the canvas that aren't part of the graph.
- [Quick actions](quick-actions.md): ready-made changes to the graph, offered in basic mode.
- [Base directories](base-directories.md): where recordings go, chosen in the main toolbar, for every writer.
- [Development](development.md): building, testing (snapshots included), versions and releases, crash reports.

The code's comments are written to be read: each module starts with what it's for and how it works, and these
docs link to them rather than repeat them.
