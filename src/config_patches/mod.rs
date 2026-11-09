//! The core's config patches: to the project's structure (the graph, its entries and edges, annotations, the
//! layout), and anything to do with every node, in the order they were added (see `crate::config_patching`). A patch is a file of its own,
//! never changed once released.

use crate::config_patching::PatchSet;

mod example;

pub const PATCHES: PatchSet = PatchSet { name: "core", version: env!("CARGO_PKG_VERSION"), patches: &[example::patch] };
