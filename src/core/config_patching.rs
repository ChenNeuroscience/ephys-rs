//! Config patches: everything saved that's read back (projects, the settings saved beside recordings, settings copied
//! to the clipboard, and what the app keeps between runs) says which version of each part of the program saved it, so
//! later versions can patch it up to date.
//!
//! Each part has a `PatchSet`: the core's (`crate::core::config_patches`), the standard components'
//! (`crate::standard_components::config_patches`), a build's own. When a part changes what it saves, it adds a `Patch`,
//! which changes what was saved before to what's saved now, as JSON, before anything's read into types. Every patch is
//! applied to everything read, and it's entirely up to the patch whether it does anything: it's given the versions that
//! saved it (`saved_before` is the usual test), and what it's looking at. Patches never need changing after, as they
//! don't depend on the types as they are now. Which sets there are, and in what order they're applied, is up to the
//! build (`Components::CONFIG_PATCHES`).

use std::collections::BTreeMap;

use serde_json::Value;

/// What a patch is looking at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SavedPart<'a> {
    /// A whole project, as saved, or beside a recording: its graph, annotations, layout and base directory. Patches are
    /// then applied to its nodes and base directory too.
    Project,
    /// A node, as saved (`{"type": ...}`), of the type named (its name in `node_set!`): in a project, or copied.
    Node(&'a str),
    /// The settings of the base directory (see `crate::core::base_directory`): in a project, or kept between runs.
    BaseDirectory,
}

/// The version of each part of the program that saved something, by its set's name. Empty for what was saved before
/// versions were.
pub type SavedVersions = BTreeMap<String, String>;

/// Changes what was saved, given the versions that saved it and what it is, if it needs to. It leaves alone what it
/// isn't about.
pub type Patch = fn(&SavedVersions, SavedPart, &mut Value);

/// The patches of a part of the program.
pub struct PatchSet {
    /// What it's called in what's saved: "core", "std", a build's name.
    pub name: &'static str,
    /// Its version, saved with what's saved: its crate's (`env!("CARGO_PKG_VERSION")`).
    pub version: &'static str,
    /// In the order they were added.
    pub patches: &'static [Patch],
}

/// Whether part `name` of the program that saved something was older than `version` (or didn't say: saved before
/// versions were, or without that part). Versions are compared a number at a time: "0.99.0" is before "0.100.0".
pub fn saved_before(saved: &SavedVersions, name: &str, version: &str) -> bool {
    let numbers = |v: &str| v.split('.').map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    saved.get(name).is_none_or(|saved| numbers(saved) < numbers(version))
}

/// A program's patch sets, in the order they're applied.
#[derive(Clone, Default)]
pub struct PatchSets(Vec<&'static PatchSet>);

impl PatchSets {
    pub fn new(sets: &'static [PatchSet]) -> Self {
        PatchSets(sets.iter().collect())
    }

    /// The versions of the parts it has, to save with what's saved.
    pub fn versions(&self) -> SavedVersions {
        self.0.iter().map(|set| (set.name.to_string(), set.version.to_string())).collect()
    }

    /// Applies every patch, set by set, to `value`, a `what` saved by `saved`.
    pub fn apply(&self, saved: &SavedVersions, what: SavedPart, value: &mut Value) {
        for patch in self.0.iter().flat_map(|set| set.patches) {
            apply(*patch, saved, what, value);
        }
    }
}

/// `patch`, on `value`, and, for a project, on its nodes and base directory.
fn apply(patch: Patch, saved: &SavedVersions, what: SavedPart, value: &mut Value) {
    patch(saved, what, value);
    if what != SavedPart::Project {
        return;
    }
    if let Some(nodes) = value.pointer_mut("/graph/nodes").and_then(Value::as_object_mut) {
        for node in nodes.values_mut().filter_map(|entry| entry.get_mut("node")) {
            if let Some(kind) = node.get("type").and_then(Value::as_str).map(str::to_string) {
                patch(saved, SavedPart::Node(&kind), node);
            }
        }
    }
    if let Some(base_directory) = value.get_mut("base_directory").filter(|b| !b.is_null()) {
        patch(saved, SavedPart::BaseDirectory, base_directory);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Notes what it was given, as a patch would change it.
    fn note(value: &mut Value, mark: &str) {
        if let Some(seen) = value.get_mut("seen").and_then(Value::as_array_mut) {
            seen.push(mark.into());
        }
    }

    fn core_project(saved: &SavedVersions, what: SavedPart, value: &mut Value) {
        if what == SavedPart::Project && saved_before(saved, "core", "1.0.0") {
            note(value, "core");
        }
    }

    fn std_writer(saved: &SavedVersions, what: SavedPart, value: &mut Value) {
        if what == SavedPart::Node("WriteLtx") && saved_before(saved, "std", "0.100.0") {
            note(value, "std writer");
        }
    }

    fn std_base_directory(saved: &SavedVersions, what: SavedPart, value: &mut Value) {
        if what == SavedPart::BaseDirectory && saved_before(saved, "std", "0.200.0") {
            note(value, "std base directory");
        }
    }

    fn build_any_node(_: &SavedVersions, what: SavedPart, value: &mut Value) {
        if let SavedPart::Node(kind) = what {
            note(value, &format!("build {kind}"));
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

    fn versions(pairs: &[(&str, &str)]) -> SavedVersions {
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
        let saved = versions(&[("std", "0.99.0")]);
        assert!(saved_before(&saved, "std", "0.100.0"));
        assert!(!saved_before(&saved, "std", "0.99.0") && !saved_before(&saved, "std", "0.9.5"));
        assert!(saved_before(&saved, "core", "0.0.1"), "not saying is older than anything");
    }

    #[test]
    fn every_patch_is_applied_to_a_project_and_its_parts_and_decides_for_itself() {
        let mut old = project();
        sets().apply(&versions(&[]), SavedPart::Project, &mut old);
        assert_eq!(old["seen"], json!(["core"]));
        assert_eq!(old["graph"]["nodes"]["1"]["node"]["seen"], json!(["std writer", "build WriteLtx"]), "in order");
        assert_eq!(old["graph"]["nodes"]["2"]["node"]["seen"], json!(["build LfpViewer"]));
        assert_eq!(old["base_directory"]["seen"], json!(["std base directory"]));

        let mut part_way = project();
        sets().apply(&versions(&[("core", "1.0.0"), ("std", "0.150.0")]), SavedPart::Project, &mut part_way);
        assert_eq!(part_way["seen"], json!([]));
        assert_eq!(part_way["graph"]["nodes"]["1"]["node"]["seen"], json!(["build WriteLtx"]));
        assert_eq!(part_way["base_directory"]["seen"], json!(["std base directory"]), "only what's after");
    }

    #[test]
    fn a_node_on_its_own_is_patched_the_same_way() {
        let mut node = json!({"type": "WriteLtx", "seen": []});
        sets().apply(&versions(&[("std", "0.1.0")]), SavedPart::Node("WriteLtx"), &mut node);
        assert_eq!(node["seen"], json!(["std writer", "build WriteLtx"]));
    }
}
