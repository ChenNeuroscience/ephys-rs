//! This build's config patches: to its own node types' settings, and its base directories', in the order they were added (see `ephys_rs::config_patching`). A patch is a file of its own,
//! never changed once released.

use ephys_rs::config_patching::PatchSet;

mod example;

pub const PATCHES: PatchSet =
    PatchSet { name: "chen-neuro", version: env!("CARGO_PKG_VERSION"), patches: &[example::patch] };
