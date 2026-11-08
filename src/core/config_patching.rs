//! Config patches: everything saved that's read back (projects, the settings saved beside recordings, settings copied
//! to the clipboard, and what the app keeps between runs) says how far each part of the program had patched what it
//! saves (its patch level), so later versions can read it.
//!
//! Each part has a `PatchSet`: the core's (`crate::core::config_patches`), the standard components'
//! (`crate::standard_components::config_patches`), a build's own. When a part changes what it saves, it adds a `Patch`,
//! which brings what's saved from the part's previous level to the new one, as JSON, before anything's read into types.
//! Patches never need changing after, as they don't depend on the types as they are now. Which sets there are, and in
//! what order they're applied, is up to the build (`Components::CONFIG_PATCHES`): reading something, each set's patches
//! are applied in turn, from the level it was saved at. A set can stop reading its oldest levels by raising `oldest` and
//! dropping their patches: what's older is then refused, saying so, as is what's newer than the program.

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

/// Brings what's saved from a set's level `n` to `n + 1`, given what it is. It leaves alone what it isn't about.
pub type Patch = fn(SavedPart, &mut Value);

/// The patches of a part of the program.
pub struct PatchSet {
    /// What it's called in what's saved: "core", "std", a build's name.
    pub name: &'static str,
    /// The oldest level it reads.
    pub oldest: u32,
    /// From `oldest`: the first brings what's saved from `oldest` to `oldest + 1`, and so on. What it saves is at the
    /// level after the last (see `level`).
    pub patches: &'static [Patch],
}

impl PatchSet {
    /// The level of what it saves.
    pub const fn level(&self) -> u32 {
        self.oldest + self.patches.len() as u32
    }
}

/// How far each set had patched what's saved. A set that isn't there is at 0: what was saved before it had patches.
pub type PatchLevels = BTreeMap<String, u32>;

/// A program's patch sets, in the order they're applied.
#[derive(Clone, Default)]
pub struct PatchSets(Vec<&'static PatchSet>);

impl PatchSets {
    pub fn new(sets: &'static [PatchSet]) -> Self {
        PatchSets(sets.iter().collect())
    }

    /// The levels of what it saves, to save with it.
    pub fn levels(&self) -> PatchLevels {
        self.0.iter().map(|set| (set.name.to_string(), set.level())).collect()
    }

    /// Brings `value`, a `what` saved at `levels`, up to date, or says why it can't be: it's too old to read, or newer
    /// than this program. Sets it doesn't know of (another build's) are left alone: what of theirs there is can't be
    /// read anyway.
    pub fn apply(&self, levels: &PatchLevels, what: SavedPart, value: &mut Value) -> Result<(), String> {
        let saved = |set: &PatchSet| levels.get(set.name).copied().unwrap_or(0);
        for set in &self.0 {
            let (saved, name) = (saved(set), set.name);
            if saved > set.level() {
                return Err(format!(
                    "It was saved by a newer version (with {name} patch level {saved}, and this one's at {}): open it in \
                     that version, or a later one",
                    set.level()
                ));
            }
            if saved < set.oldest {
                return Err(format!(
                    "It was saved by a version too old for this one (with {name} patch level {saved}, and this one reads \
                     from {}): open and save it in a version between, first",
                    set.oldest
                ));
            }
        }
        for set in &self.0 {
            for patch in &set.patches[(saved(set) - set.oldest) as usize..] {
                apply(*patch, what, value);
            }
        }
        Ok(())
    }
}

/// `patch`, on `value`, and, for a project, on its nodes and base directory.
fn apply(patch: Patch, what: SavedPart, value: &mut Value) {
    patch(what, value);
    if what != SavedPart::Project {
        return;
    }
    if let Some(nodes) = value.pointer_mut("/graph/nodes").and_then(Value::as_object_mut) {
        for node in nodes.values_mut().filter_map(|entry| entry.get_mut("node")) {
            if let Some(kind) = node.get("type").and_then(Value::as_str).map(str::to_string) {
                patch(SavedPart::Node(&kind), node);
            }
        }
    }
    if let Some(base_directory) = value.get_mut("base_directory").filter(|b| !b.is_null()) {
        patch(SavedPart::BaseDirectory, base_directory);
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

    fn core_project(what: SavedPart, value: &mut Value) {
        if what == SavedPart::Project {
            note(value, "core 0→1");
        }
    }

    fn std_writer(what: SavedPart, value: &mut Value) {
        if what == SavedPart::Node("WriteLtx") {
            note(value, "std 3→4");
        }
    }

    fn std_base_directory(what: SavedPart, value: &mut Value) {
        if what == SavedPart::BaseDirectory {
            note(value, "std 4→5");
        }
    }

    fn build_any_node(what: SavedPart, value: &mut Value) {
        if let SavedPart::Node(kind) = what {
            note(value, &format!("build {kind}"));
        }
    }

    static SETS: [PatchSet; 3] = [
        PatchSet { name: "core", oldest: 0, patches: &[core_project] },
        PatchSet { name: "std", oldest: 3, patches: &[std_writer, std_base_directory] },
        PatchSet { name: "build", oldest: 0, patches: &[build_any_node] },
    ];

    fn sets() -> PatchSets {
        PatchSets::new(&SETS)
    }

    fn levels(pairs: &[(&str, u32)]) -> PatchLevels {
        pairs.iter().map(|(name, n)| (name.to_string(), *n)).collect()
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
    fn what_is_saved_says_the_level_of_each_set() {
        assert_eq!(sets().levels(), levels(&[("core", 1), ("std", 5), ("build", 1)]));
        assert_eq!(PatchSets::default().levels(), levels(&[]), "only what the build says");
    }

    #[test]
    fn a_project_is_brought_up_to_date_set_by_set_from_the_level_it_was_at() {
        let mut old = project();
        sets().apply(&levels(&[("std", 3)]), SavedPart::Project, &mut old).unwrap();
        assert_eq!(old["seen"], json!(["core 0→1"]), "in the order given: missing is 0");
        assert_eq!(old["graph"]["nodes"]["1"]["node"]["seen"], json!(["std 3→4", "build WriteLtx"]));
        assert_eq!(old["graph"]["nodes"]["2"]["node"]["seen"], json!(["build LfpViewer"]));
        assert_eq!(old["base_directory"]["seen"], json!(["std 4→5"]));

        let mut current = project();
        sets().apply(&sets().levels(), SavedPart::Project, &mut current).unwrap();
        assert_eq!(current, project(), "nothing to do");
        let mut part_way = project();
        sets().apply(&levels(&[("core", 1), ("std", 4), ("build", 1)]), SavedPart::Project, &mut part_way).unwrap();
        assert_eq!(part_way["base_directory"]["seen"], json!(["std 4→5"]), "only what's after");
        assert_eq!(part_way["graph"]["nodes"]["1"]["node"]["seen"], json!([]));
    }

    #[test]
    fn a_node_on_its_own_is_brought_up_to_date_the_same_way() {
        let mut node = json!({"type": "WriteLtx", "seen": []});
        sets().apply(&levels(&[("std", 3)]), SavedPart::Node("WriteLtx"), &mut node).unwrap();
        assert_eq!(node["seen"], json!(["std 3→4", "build WriteLtx"]));
    }

    #[test]
    fn what_is_too_old_or_too_new_is_refused_saying_so() {
        let mut value = project();
        let newer = sets().apply(&levels(&[("std", 6)]), SavedPart::Project, &mut value).unwrap_err();
        assert!(newer.contains("newer version") && newer.contains("std patch level 6"), "{newer}");
        let older = sets().apply(&levels(&[("std", 2)]), SavedPart::Project, &mut value).unwrap_err();
        assert!(older.contains("too old") && older.contains("std patch level 2"), "{older}");
        assert_eq!(value, project(), "untouched");
        sets().apply(&levels(&[("std", 3), ("another build", 9)]), SavedPart::Project, &mut value).unwrap();
    }
}
