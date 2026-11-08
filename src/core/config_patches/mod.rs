//! The core's config patches: to the project's structure (the graph, its entries and edges, annotations, the
//! layout), and anything to do with every node, in the order they were added: each brings what's saved from one level to the next (see
//! `crate::core::config_patching`). A patch is a file of its own, never changed once released.

use crate::core::config_patching::PatchSet;

mod example;

pub const PATCHES: PatchSet = PatchSet { name: "core", oldest: 0, patches: &[example::patch] };
