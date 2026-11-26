//! Every piece of config the app reads back records which version of each part of the program wrote it, so later
//! versions can patch it up to date. That covers saved files (projects, and the settings saved beside recordings),
//! settings pasted from the clipboard, quick actions (shaped as projects: see `crate::quick_action`), and the base
//! directory's settings in the app's stored preferences.
//!
//! Each part of the program has a `PatchSet`: the core's (`crate::config_patches`), the standard components'
//! (`crate::standard::config_patches`), and a build's own. When a part changes what it saves, it adds a `Patch`, which
//! converts what was saved before into what's saved now, as JSON, before anything is read into types. A patch is a
//! script: it receives the incoming config (`IncomingConfig`), always shaped as a project
//! (`{"graph": {"nodes": {...}}, "base_directory": ...}`), and changes it however it needs to. Config that isn't a
//! project is wrapped in one: copied nodes go in its graph, and the base directory's settings from the app's
//! preferences become its base directory, with nothing else. Patches never need changing afterwards, because they
//! don't depend on the program's types as they are now.
//!
//! Each patch declares the version it was added in (`Patch::since`). The app only applies a patch to config written
//! before that version of the patch's part (or written without saying). The app applies patches in the order they
//! were written, across all the sets, so each patch sees the config in the shape it was written for: by ephys-rs
//! version, then core before std before the build, then in the order each set lists them (see `Patch::build` for how
//! a build's patches are placed). Which sets there are, and their order, is up to the build
//! (`Components::CONFIG_PATCHES`).

use std::collections::BTreeMap;

use serde_json::Value;

/// What's written (saved or copied) says the version of each part of the program that wrote it, by its set's name. What
/// was written before versions were says none.
pub type Versions = BTreeMap<String, String>;

/// A patch is given config coming in: a saved file (a project, or the settings saved beside a recording), settings
/// pasted from the clipboard (copied, maybe, to lab notes, and back, long after), or the base directory's settings from
/// the app's stored preferences.
pub struct IncomingConfig {
    /// The versions that wrote the config.
    pub versions: Versions,
    /// The config, shaped as a project: `{"graph": {"nodes": {...}}, "base_directory": ...}`.
    pub json: Value,
}

/// A version's numbers, for comparing versions a number at a time: "0.99.0" is before "0.100.0".
fn numbers(version: &str) -> Vec<u64> {
    version.split('.').map(|n| n.parse().unwrap_or(0)).collect()
}

impl IncomingConfig {
    /// Whether part `name` of the program that wrote the config was older than `version`. Config that doesn't say
    /// (written before versions were recorded, or without that part) counts as older.
    fn written_before(&self, name: &str, version: &str) -> bool {
        self.versions.get(name).is_none_or(|written| numbers(written) < numbers(version))
    }

    /// A patch changes the config's nodes (as saved: `{"type": ...}`), if it has any, as it likes, through this.
    pub fn nodes(&mut self) -> impl Iterator<Item = &mut Value> {
        let nodes = self.json.pointer_mut("/graph/nodes").and_then(Value::as_object_mut);
        nodes.into_iter().flat_map(|nodes| nodes.values_mut().filter_map(|entry| entry.get_mut("node")))
    }
}

/// A change to incoming config, converting what one part of the program wrote before version `since` into what the
/// part writes now.
pub struct Patch {
    /// The first version of the patch's part that writes config in the new form. The app applies the patch only to
    /// config written by an earlier version of the part, or by a program that didn't record the part's version.
    pub since: &'static str,
    /// The ephys-rs version that places the patch in the order patches are applied.
    pub ephys_rs: &'static str,
    /// Converts the config.
    pub apply: fn(&mut IncomingConfig),
}

impl Patch {
    /// A core or standard patch. The core and the standard components share ephys-rs's version, so `since` also
    /// places the patch in the order.
    pub const fn new(since: &'static str, apply: fn(&mut IncomingConfig)) -> Self {
        Patch { since, ephys_rs: since, apply }
    }

    /// A build's patch. `since` is a version of the build. A build's versions can't be compared with ephys-rs's, so
    /// `ephys_rs` is the version of ephys-rs the build used when the patch was written, as a literal that never
    /// changes. The patch runs after every core and standard patch up to that version, and before any later ones.
    pub const fn build(since: &'static str, ephys_rs: &'static str, apply: fn(&mut IncomingConfig)) -> Self {
        Patch { since, ephys_rs, apply }
    }

    /// Applies the patch to `config` if part `set` (the name of the patch's set) of the program that wrote the config
    /// was older than `since`.
    pub fn apply_to(&self, set: &str, config: &mut IncomingConfig) {
        if config.written_before(set, self.since) {
            (self.apply)(config);
        }
    }
}

/// The patches of one part of the program.
pub struct PatchSet {
    /// The part's name, which written config records its version under: "core", "std", or a build's name.
    pub name: &'static str,
    /// The part's version, which the app records with what it writes: the crate's (`env!("CARGO_PKG_VERSION")`).
    pub version: &'static str,
    /// The part's patches, in the order they were added.
    pub patches: &'static [Patch],
}

/// A program's patch sets, and every patch in the order the app applies them: by ephys-rs version (`Patch::ephys_rs`),
/// then by the sets' order (core, std, then the build, as `Components::CONFIG_PATCHES` lists them), then in the order
/// each set lists them.
#[derive(Clone, Default)]
pub struct PatchSets {
    sets: Vec<&'static PatchSet>,
    /// Each patch with the name of its set, in the order the app applies them.
    ordered: Vec<(&'static str, &'static Patch)>,
}

impl PatchSets {
    pub fn new(sets: &'static [PatchSet]) -> Self {
        for set in sets {
            let in_order = set.patches.windows(2).all(|pair| {
                let [a, b] = pair else { return true };
                numbers(a.since) <= numbers(b.since) && numbers(a.ephys_rs) <= numbers(b.ephys_rs)
            });
            assert!(in_order, "the {} patches must be listed in the order they were added", set.name);
        }
        let mut ordered: Vec<(usize, &'static str, &'static Patch)> =
            (sets.iter().enumerate()).flat_map(|(i, set)| set.patches.iter().map(move |p| (i, set.name, p))).collect();
        // A stable sort, so patches of the same set and version stay in the order the set lists them.
        ordered.sort_by(|a, b| numbers(a.2.ephys_rs).cmp(&numbers(b.2.ephys_rs)).then(a.0.cmp(&b.0)));
        let ordered = ordered.into_iter().map(|(_, name, patch)| (name, patch)).collect();
        PatchSets { sets: sets.iter().collect(), ordered }
    }

    /// The versions of the program's parts, which the app records with what it writes.
    pub fn versions(&self) -> Versions {
        self.sets.iter().map(|set| (set.name.to_string(), set.version.to_string())).collect()
    }

    /// Applies, in order, every patch for a version of its part later than the one that wrote `config`.
    pub fn apply(&self, config: &mut IncomingConfig) {
        for (set, patch) in &self.ordered {
            patch.apply_to(set, config);
        }
    }

    /// Applies the patches (see `apply`) to `project`, which was written by `versions`.
    pub fn apply_to_project(&self, versions: &Versions, project: &mut Value) {
        let mut config = IncomingConfig { versions: versions.clone(), json: project.take() };
        self.apply(&mut config);
        *project = config.json;
    }

    /// Applies the patches (see `apply`) to `nodes`, each as saved (`{"type": ...}`) and written by `versions`. The
    /// nodes are put in a project's graph for the patches.
    pub fn apply_to_nodes(&self, versions: &Versions, nodes: &mut [Value]) {
        let entries: serde_json::Map<String, Value> = nodes
            .iter_mut()
            .enumerate()
            .map(|(i, node)| (i.to_string(), serde_json::json!({ "node": node.take() })))
            .collect();
        let mut project = serde_json::json!({ "graph": { "nodes": entries } });
        self.apply_to_project(versions, &mut project);
        for (i, node) in nodes.iter_mut().enumerate() {
            if let Some(patched) = project.pointer_mut(&format!("/graph/nodes/{i}/node")) {
                *node = patched.take();
            }
        }
    }

    /// Applies the patches (see `apply`) to `base_directory`, which was written by `versions`. The base directory's
    /// settings are put in a project for the patches.
    pub fn apply_to_base_directory(&self, versions: &Versions, base_directory: &mut Value) {
        let mut project = serde_json::json!({ "base_directory": base_directory.take() });
        self.apply_to_project(versions, &mut project);
        *base_directory = project["base_directory"].take();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Records that the patch `mark` ran, in `value`'s list of what has.
    fn note(value: &mut Value, mark: &str) {
        if let Some(seen) = value.get_mut("seen").and_then(Value::as_array_mut) {
            seen.push(mark.into());
        }
    }

    fn core_tie(config: &mut IncomingConfig) {
        note(&mut config.json, "core 0.200");
    }

    fn core_late(config: &mut IncomingConfig) {
        note(&mut config.json, "core 0.250");
    }

    fn std_writer(config: &mut IncomingConfig) {
        note(&mut config.json, "std 0.100");
        for node in config.nodes().filter(|n| n["type"] == "WriteLtx") {
            note(node, "std writer");
        }
    }

    fn std_base_directory(config: &mut IncomingConfig) {
        note(&mut config.json, "std 0.200");
        note(&mut config.json["base_directory"], "std base directory");
    }

    fn build_any_node(config: &mut IncomingConfig) {
        note(&mut config.json, "build 2.0");
        for node in config.nodes() {
            let kind = node["type"].as_str().unwrap_or_default().to_string();
            note(node, &format!("build {kind}"));
        }
    }

    static SETS: [PatchSet; 3] = [
        PatchSet {
            name: "core",
            version: "0.300.0",
            patches: &[Patch::new("0.200.0", core_tie), Patch::new("0.250.0", core_late)],
        },
        PatchSet {
            name: "std",
            version: "0.300.0",
            patches: &[Patch::new("0.100.0", std_writer), Patch::new("0.200.0", std_base_directory)],
        },
        PatchSet { name: "build", version: "2.0.0", patches: &[Patch::build("2.0.0", "0.200.0", build_any_node)] },
    ];

    fn sets() -> PatchSets {
        PatchSets::new(&SETS)
    }

    fn versions(pairs: &[(&str, &str)]) -> Versions {
        pairs.iter().map(|(name, v)| (name.to_string(), v.to_string())).collect()
    }

    fn project() -> Value {
        json!({
            "seen": [],
            "graph": {"nodes": {
                "1": {"label": "a", "node": {"type": "WriteLtx", "seen": []}},
                "2": {"label": "b", "node": {"type": "ContinuousViewer", "seen": []}},
            }},
            "base_directory": {"seen": []},
        })
    }

    #[test]
    fn what_is_saved_says_the_version_of_each_part() {
        assert_eq!(sets().versions(), versions(&[("core", "0.300.0"), ("std", "0.300.0"), ("build", "2.0.0")]));
        assert_eq!(PatchSets::default().versions(), versions(&[]), "only what the build says");
    }

    #[test]
    fn versions_are_compared_a_number_at_a_time() {
        let config = IncomingConfig { versions: versions(&[("std", "0.99.0")]), json: Value::Null };
        assert!(config.written_before("std", "0.100.0"));
        assert!(!config.written_before("std", "0.99.0") && !config.written_before("std", "0.9.5"));
        assert!(config.written_before("core", "0.0.1"), "not saying is older than anything");
    }

    /// Patches run in the order they were written, across the sets: by ephys-rs version, then core before std before
    /// the build. The build's patch was written against ephys-rs 0.200, so it runs after the core and standard patches
    /// up to 0.200, and before the core's 0.250 patch.
    #[test]
    fn patches_are_applied_in_the_order_they_were_written() {
        let mut old = project();
        sets().apply_to_project(&versions(&[]), &mut old);
        assert_eq!(old["seen"], json!(["std 0.100", "core 0.200", "std 0.200", "build 2.0", "core 0.250"]));
        assert_eq!(old["graph"]["nodes"]["1"]["node"]["seen"], json!(["std writer", "build WriteLtx"]));
        assert_eq!(old["graph"]["nodes"]["2"]["node"]["seen"], json!(["build ContinuousViewer"]));
        assert_eq!(old["base_directory"]["seen"], json!(["std base directory"]));
    }

    /// A patch only applies to config written before the version of its own part that added it.
    #[test]
    fn only_patches_since_what_wrote_the_config_apply() {
        let mut part_way = project();
        let written = versions(&[("core", "0.250.0"), ("std", "0.150.0"), ("build", "2.0.0")]);
        sets().apply_to_project(&written, &mut part_way);
        assert_eq!(part_way["seen"], json!(["std 0.200"]));
        assert_eq!(part_way["base_directory"]["seen"], json!(["std base directory"]));
        assert_eq!(part_way["graph"]["nodes"]["1"]["node"]["seen"], json!([]));
    }

    #[test]
    fn nodes_and_a_base_directory_on_their_own_are_patched_in_a_project() {
        let mut nodes = [json!({"type": "WriteLtx", "seen": []}), json!({"type": "ContinuousViewer", "seen": []})];
        sets().apply_to_nodes(&versions(&[("std", "0.100.0")]), &mut nodes);
        assert_eq!(nodes[0]["seen"], json!(["build WriteLtx"]));
        assert_eq!(nodes[1]["seen"], json!(["build ContinuousViewer"]));
        let mut base_directory = json!({"seen": []});
        sets().apply_to_base_directory(&versions(&[]), &mut base_directory);
        assert_eq!(base_directory["seen"], json!(["std base directory"]));
    }
}
