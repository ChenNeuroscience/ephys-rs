//! The standard components' config patches: to their node types' settings, and the base directories', in the order they were added (see `crate::core::config_patching`). A patch is a file of its own,
//! never changed once released. There are none yet: `docs/development.md` shows what one looks like.

use crate::core::config_patching::PatchSet;

pub const PATCHES: PatchSet = PatchSet { name: "std", version: env!("CARGO_PKG_VERSION"), patches: &[] };
