//! This build's config patches: to its own node types' settings, and its base directories', in the order they were added: each brings what's saved from one level to the next (see
//! `ephys_rs::core::config_patching`). A patch is a file of its own, never changed once released.

use ephys_rs::core::config_patching::PatchSet;

mod example;

pub const PATCHES: PatchSet = PatchSet { name: "chen-neuro", oldest: 0, patches: &[example::patch] };
