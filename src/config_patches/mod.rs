//! The core's config patches, which change the project's structure (the graph, its entries and edges, annotations,
//! the layout) and anything to do with every node (see `crate::config_patching`). Each patch is a file of its own,
//! listed in `PATCHES` in the order the patches were added, and never changed once released. `example` shows the
//! shape of a patch.

use crate::config_patching::PatchSet;

mod edge_roles;
mod example;
mod quick_action_edges;

pub const PATCHES: PatchSet = PatchSet {
    name: "core",
    version: env!("CARGO_PKG_VERSION"),
    patches: &[edge_roles::PATCH, quick_action_edges::PATCH],
};
