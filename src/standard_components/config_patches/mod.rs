//! The standard components' config patches: to their node types' settings, and the base directories', in the order they were added: each brings what's saved from one level to the next (see
//! `crate::core::config_patching`). A patch is a file of its own, never changed once released.

use crate::core::config_patching::PatchSet;

mod example;

pub const PATCHES: PatchSet = PatchSet { name: "std", oldest: 0, patches: &[example::patch] };

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::core::config_patching::{PatchLevels, PatchSet, PatchSets, SavedPart};

    /// Something saved before there were patches is brought up to date through the example, unchanged.
    #[test]
    fn the_example_patch_changes_nothing() {
        static SETS: [PatchSet; 2] = [crate::core::config_patches::PATCHES, super::PATCHES];
        let sets = PatchSets::new(&SETS);
        assert_eq!(sets.levels(), PatchLevels::from([("core".into(), 1), ("std".into(), 1)]));
        let saved = json!({ "graph": { "nodes": { "1": { "node": { "type": "SpikeDetector", "threshold": 4 } } } } });
        let mut value = saved.clone();
        sets.apply(&PatchLevels::new(), SavedPart::Project, &mut value).unwrap();
        assert_eq!(value, saved);
    }
}
