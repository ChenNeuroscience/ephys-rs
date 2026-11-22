//! This build's config patches, which change its own node types' settings and its base directories' (see
//! `ephys_rs::config_patching`). Each patch is a file of its own, listed in `PATCHES` in the order the patches were
//! added, and never changed once released. `example` shows the shape of a patch.

use ephys_rs::config_patching::PatchSet;

mod edge_port_ids;
mod example;

pub const PATCHES: PatchSet =
    PatchSet { name: "chen-neuro", version: env!("CARGO_PKG_VERSION"), patches: &[edge_port_ids::PATCH] };
