//! All the config the app reads back says which version of each part of the program wrote it, so later versions can
//! patch it up to date: saved files (projects, and the settings saved beside recordings), settings pasted
//! from the clipboard, and the base directory's settings in the app's stored preferences.
//!
//! Each part has a `PatchSet`: the core's (`crate::config_patches`), the standard components'
//! (`crate::standard::config_patches`), a build's own. When a part changes what it saves, it adds a `Patch`, which
//! changes what was saved before to what's saved now, as JSON, before anything's read into types. Think of a patch as a
//! script: it's given the config coming in (`IncomingConfig`): the versions that wrote it (`written_before` is the
//! usual test), and the JSON, always shaped as a project (`{"graph": {"nodes": {...}}, "base_directory": ...}`), and
//! does as it likes. What isn't a project is put in one for it: copied nodes in its graph, the base directory's
//! settings from the app's preferences as its base directory, with nothing else. Every patch is applied to everything
//! read. Patches never need changing after, as they don't depend on the types as they are now. Which sets there are,
//! and in what order they're applied, is up to the build (`Components::CONFIG_PATCHES`).

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

impl IncomingConfig {
    /// A patch usually checks whether part `name` of the program that wrote the config was older than `version` (or
    /// didn't say: written before versions were, or without that part). Versions are compared a number at a time:
    /// "0.99.0" is before "0.100.0".
    pub fn written_before(&self, name: &str, version: &str) -> bool {
        let numbers = |v: &str| v.split('.').map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
        self.versions.get(name).is_none_or(|written| numbers(written) < numbers(version))
    }

    /// A patch changes the config's nodes (as saved: `{"type": ...}`), if it has any, as it likes, through this.
    pub fn nodes(&mut self) -> impl Iterator<Item = &mut Value> {
        let nodes = self.json.pointer_mut("/graph/nodes").and_then(Value::as_object_mut);
        nodes.into_iter().flat_map(|nodes| nodes.values_mut().filter_map(|entry| entry.get_mut("node")))
    }
}

/// A patch changes config coming in from what was written before to what's written now, if it needs to.
pub type Patch = fn(&mut IncomingConfig);

/// Each part of the program has a set of patches.
pub struct PatchSet {
    /// What's written names the part this: "core", "std", a build's name.
    pub name: &'static str,
    /// The part's version, written with what's written: its crate's (`env!("CARGO_PKG_VERSION")`).
    pub version: &'static str,
    /// The part's patches, in the order they were added.
    pub patches: &'static [Patch],
}

/// The app applies a program's patch sets in this order.
#[derive(Clone, Default)]
pub struct PatchSets(Vec<&'static PatchSet>);

impl PatchSets {
    pub fn new(sets: &'static [PatchSet]) -> Self {
        PatchSets(sets.iter().collect())
    }

    /// The app writes the versions of the program's parts with what it writes, from this.
    pub fn versions(&self) -> Versions {
        self.0.iter().map(|set| (set.name.to_string(), set.version.to_string())).collect()
    }

    /// The app applies every patch, set by set, to `config` with this.
    pub fn apply(&self, config: &mut IncomingConfig) {
        for patch in self.0.iter().flat_map(|set| set.patches) {
            patch(config);
        }
    }

    /// The app applies every patch to `project`, written by `versions`, with this.
    pub fn apply_to_project(&self, versions: &Versions, project: &mut Value) {
        let mut config = IncomingConfig { versions: versions.clone(), json: project.take() };
        self.apply(&mut config);
        *project = config.json;
    }

    /// The app applies every patch to `nodes` (each as saved: `{"type": ...}`), written by `versions`, with this, put
    /// in a project's graph for them.
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

    /// The app applies every patch to `base_directory`, written by `versions`, with this, put in a project for them.
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

    /// The test's patch notes what it was given, as a patch would change it.
    fn note(value: &mut Value, mark: &str) {
        if let Some(seen) = value.get_mut("seen").and_then(Value::as_array_mut) {
            seen.push(mark.into());
        }
    }

    fn core_project(config: &mut IncomingConfig) {
        if config.written_before("core", "1.0.0") {
            note(&mut config.json, "core");
        }
    }

    fn std_writer(config: &mut IncomingConfig) {
        if config.written_before("std", "0.100.0") {
            for node in config.nodes().filter(|n| n["type"] == "WriteLtx") {
                note(node, "std writer");
            }
        }
    }

    fn std_base_directory(config: &mut IncomingConfig) {
        if config.written_before("std", "0.200.0") {
            note(&mut config.json["base_directory"], "std base directory");
        }
    }

    fn build_any_node(config: &mut IncomingConfig) {
        for node in config.nodes() {
            let kind = node["type"].as_str().unwrap_or_default().to_string();
            note(node, &format!("build {kind}"));
        }
    }

    static SETS: [PatchSet; 3] = [
        PatchSet { name: "core", version: "1.0.0", patches: &[core_project] },
        PatchSet { name: "std", version: "0.200.0", patches: &[std_writer, std_base_directory] },
        PatchSet { name: "build", version: "2.0.0", patches: &[build_any_node] },
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
                "2": {"label": "b", "node": {"type": "LfpViewer", "seen": []}},
            }},
            "base_directory": {"seen": []},
        })
    }

    #[test]
    fn what_is_saved_says_the_version_of_each_part() {
        assert_eq!(sets().versions(), versions(&[("core", "1.0.0"), ("std", "0.200.0"), ("build", "2.0.0")]));
        assert_eq!(PatchSets::default().versions(), versions(&[]), "only what the build says");
    }

    #[test]
    fn versions_are_compared_a_number_at_a_time() {
        let config = IncomingConfig { versions: versions(&[("std", "0.99.0")]), json: Value::Null };
        assert!(config.written_before("std", "0.100.0"));
        assert!(!config.written_before("std", "0.99.0") && !config.written_before("std", "0.9.5"));
        assert!(config.written_before("core", "0.0.1"), "not saying is older than anything");
    }

    #[test]
    fn every_patch_is_applied_to_a_project_and_decides_for_itself() {
        let mut old = project();
        sets().apply_to_project(&versions(&[]), &mut old);
        assert_eq!(old["seen"], json!(["core"]));
        assert_eq!(old["graph"]["nodes"]["1"]["node"]["seen"], json!(["std writer", "build WriteLtx"]), "in order");
        assert_eq!(old["graph"]["nodes"]["2"]["node"]["seen"], json!(["build LfpViewer"]));
        assert_eq!(old["base_directory"]["seen"], json!(["std base directory"]));

        let mut part_way = project();
        sets().apply_to_project(&versions(&[("core", "1.0.0"), ("std", "0.150.0")]), &mut part_way);
        assert_eq!(part_way["seen"], json!([]));
        assert_eq!(part_way["graph"]["nodes"]["1"]["node"]["seen"], json!(["build WriteLtx"]));
        assert_eq!(part_way["base_directory"]["seen"], json!(["std base directory"]), "only what's after");
    }

    #[test]
    fn nodes_and_a_base_directory_on_their_own_are_patched_in_a_project() {
        let mut nodes = [json!({"type": "WriteLtx", "seen": []}), json!({"type": "LfpViewer", "seen": []})];
        sets().apply_to_nodes(&versions(&[("std", "0.1.0")]), &mut nodes);
        assert_eq!(nodes[0]["seen"], json!(["std writer", "build WriteLtx"]));
        assert_eq!(nodes[1]["seen"], json!(["build LfpViewer"]));
        let mut base_directory = json!({"seen": []});
        sets().apply_to_base_directory(&versions(&[]), &mut base_directory);
        assert_eq!(base_directory["seen"], json!(["std base directory"]));
    }
}
