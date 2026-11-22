//! The standard components' config patches, which change the standard node types' settings and the base directories'
//! (see `ephys_rs::config_patching`). Each patch is a file of its own, listed in `PATCHES` in the order the patches
//! were added, and never changed once released. `example` shows the shape of a patch.

use ephys_rs::config_patching::PatchSet;

mod auto_detect_settings;
mod board_outputs;
mod continuous_viewer;
mod continuous_viewer_spectra;
mod edge_port_ids;
mod example;
mod gains_amplify;
mod write_ltx_val_ins;

pub const PATCHES: PatchSet = PatchSet {
    name: "std",
    version: env!("CARGO_PKG_VERSION"),
    patches: &[
        gains_amplify::PATCH,
        auto_detect_settings::PATCH,
        write_ltx_val_ins::PATCH,
        board_outputs::PATCH,
        edge_port_ids::PATCH,
        continuous_viewer::PATCH,
        continuous_viewer_spectra::PATCH,
    ],
};

#[cfg(test)]
mod tests {
    use serde_json::json;

    use ephys_rs::config_patching::{PatchSet, PatchSets, Versions};

    /// A Spike Detector saved before there were versions comes through every patch unchanged, as none of them
    /// touches its settings.
    #[test]
    fn a_spike_detector_saved_before_versions_is_unchanged() {
        static SETS: [PatchSet; 2] = [ephys_rs::config_patches::PATCHES, super::PATCHES];
        let sets = PatchSets::new(&SETS);
        assert_eq!(sets.versions().keys().collect::<Vec<_>>(), ["core", "std"]);
        let saved = json!({ "graph": { "nodes": { "1": { "node": { "type": "SpikeDetector", "threshold": 4 } } } } });
        let mut value = saved.clone();
        sets.apply_to_project(&Versions::new(), &mut value);
        assert_eq!(value, saved);
    }
}
