//! The standard components' config patches: to their node types' settings, and the base directories', in the order they were added (see `crate::core::config_patching`). A patch is a file of its own,
//! never changed once released.

use crate::core::config_patching::PatchSet;

mod example;

pub const PATCHES: PatchSet = PatchSet { name: "std", version: env!("CARGO_PKG_VERSION"), patches: &[example::patch] };

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::core::config_patching::{PatchSet, PatchSets, SavedVersions};

    /// Something saved before there were versions is patched through the example, unchanged.
    #[test]
    fn the_example_patch_changes_nothing() {
        static SETS: [PatchSet; 2] = [crate::core::config_patches::PATCHES, super::PATCHES];
        let sets = PatchSets::new(&SETS);
        assert_eq!(sets.versions().keys().collect::<Vec<_>>(), ["core", "std"]);
        let saved = json!({ "graph": { "nodes": { "1": { "node": { "type": "SpikeDetector", "threshold": 4 } } } } });
        let mut value = saved.clone();
        sets.apply(&SavedVersions::new(), &mut value);
        assert_eq!(value, saved);
    }
}
