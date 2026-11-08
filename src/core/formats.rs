//! Formats: everything saved that's read back (projects, the settings saved beside recordings, settings copied to the
//! clipboard, and what the app keeps between runs) says which format of each part of the program wrote it, so later
//! versions can read it.
//!
//! Each part is a `Layer`: the core's (`CORE`), the standard components', a build's own (see `Components::FORMATS`).
//! When a part changes what it saves, it adds an `Upgrade`, which brings what's saved from its previous format to the new
//! one, as JSON, before anything's read into types. Steps never need changing after, as they don't depend on the types
//! as they are now. Reading something, the core's steps go first, then each other layer's, in order; each layer from
//! the format it was saved in. A layer can stop reading its oldest formats by raising `oldest` and dropping their steps:
//! what's older is then refused, saying so, as is what's newer than the program.

use std::collections::BTreeMap;

use serde_json::Value;

/// What a step is looking at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SavedPart<'a> {
    /// A whole project, as saved, or beside a recording: its graph, annotations, layout and base directory. Its steps
    /// are then run on its nodes and base directory too.
    Project,
    /// A node, as saved (`{"type": ...}`), of the type named (its name in `node_set!`): in a project, or copied.
    Node(&'a str),
    /// The settings of the base directory (see `crate::core::base_directory`): in a project, or kept between runs.
    BaseDirectory,
}

/// Brings what's saved from a layer's format `n` to `n + 1`, given what it is. It leaves alone what it isn't about.
pub type Upgrade = fn(SavedPart, &mut Value);

/// The formats of a part of the program.
pub struct Layer {
    /// What it's called in what's saved: "core", "std", a build's name.
    pub name: &'static str,
    /// The oldest format it reads.
    pub oldest: u32,
    /// From `oldest`: the first brings what's saved from `oldest` to `oldest + 1`, and so on. It writes the format
    /// after the last (see `current`).
    pub steps: &'static [Upgrade],
}

impl Layer {
    /// The format it writes.
    pub const fn current(&self) -> u32 {
        self.oldest + self.steps.len() as u32
    }
}

/// The core's: the project's structure (the graph, its entries and edges, annotations, the layout), and anything to do
/// with every node.
pub const CORE: Layer = Layer { name: "core", oldest: 0, steps: &[] };

/// Which format of each layer wrote something. A layer that isn't there is at 0: what was saved before formats were.
pub type Formats = BTreeMap<String, u32>;

/// The layers of a program: the core's first, then those it's given.
#[derive(Clone)]
pub struct Layers(Vec<&'static Layer>);

impl Default for Layers {
    /// Just the core's.
    fn default() -> Self {
        Layers::new(&[])
    }
}

impl Layers {
    pub fn new(others: &'static [Layer]) -> Self {
        Layers(std::iter::once(&CORE).chain(others).collect())
    }

    /// The formats it writes, to save with what's saved.
    pub fn current(&self) -> Formats {
        self.0.iter().map(|layer| (layer.name.to_string(), layer.current())).collect()
    }

    /// Brings `value`, a `what` saved in `formats`, up to date, or says why it can't be: it's from a format too old to
    /// read, or newer than this program. Layers it doesn't know of (another build's) are left alone: what of theirs
    /// there is can't be read anyway.
    pub fn upgrade(&self, formats: &Formats, what: SavedPart, value: &mut Value) -> Result<(), String> {
        let saved = |layer: &Layer| formats.get(layer.name).copied().unwrap_or(0);
        for layer in &self.0 {
            let (saved, name) = (saved(layer), layer.name);
            if saved > layer.current() {
                return Err(format!(
                    "It was saved by a newer version (in {name} format {saved}, and this one reads up to {}): open it in that \
                     version, or a later one",
                    layer.current()
                ));
            }
            if saved < layer.oldest {
                return Err(format!(
                    "It was saved by a version too old for this one (in {name} format {saved}, and this one reads from {}): \
                     open and save it in a version between, first",
                    layer.oldest
                ));
            }
        }
        for layer in &self.0 {
            for step in &layer.steps[(saved(layer) - layer.oldest) as usize..] {
                apply(*step, what, value);
            }
        }
        Ok(())
    }
}

/// `step`, on `value`, and, for a project, on its nodes and base directory.
fn apply(step: Upgrade, what: SavedPart, value: &mut Value) {
    step(what, value);
    if what != SavedPart::Project {
        return;
    }
    if let Some(nodes) = value.pointer_mut("/graph/nodes").and_then(Value::as_object_mut) {
        for node in nodes.values_mut().filter_map(|entry| entry.get_mut("node")) {
            if let Some(kind) = node.get("type").and_then(Value::as_str).map(str::to_string) {
                step(SavedPart::Node(&kind), node);
            }
        }
    }
    if let Some(base_directory) = value.get_mut("base_directory").filter(|b| !b.is_null()) {
        step(SavedPart::BaseDirectory, base_directory);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Notes what it was given, as a step would change it.
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

    const STD: Layer = Layer { name: "std", oldest: 3, steps: &[std_writer, std_base_directory] };
    const BUILD: Layer = Layer { name: "build", oldest: 0, steps: &[build_any_node] };
    static OTHERS: [Layer; 2] = [STD, BUILD];

    fn layers() -> Layers {
        Layers(vec![&Layer { name: "core", oldest: 0, steps: &[core_project] }, &OTHERS[0], &OTHERS[1]])
    }

    fn formats(pairs: &[(&str, u32)]) -> Formats {
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
    fn what_is_saved_says_which_format_of_each_layer_wrote_it() {
        assert_eq!(layers().current(), formats(&[("core", 1), ("std", 5), ("build", 1)]));
        assert_eq!(Layers::default().current(), formats(&[("core", CORE.current())]), "just the core's");
    }

    #[test]
    fn a_project_is_brought_up_to_date_layer_by_layer_from_the_format_it_was_in() {
        let mut old = project();
        layers().upgrade(&formats(&[("std", 3)]), SavedPart::Project, &mut old).unwrap();
        assert_eq!(old["seen"], json!(["core 0→1"]), "the core's first: missing is 0");
        assert_eq!(old["graph"]["nodes"]["1"]["node"]["seen"], json!(["std 3→4", "build WriteLtx"]));
        assert_eq!(old["graph"]["nodes"]["2"]["node"]["seen"], json!(["build LfpViewer"]));
        assert_eq!(old["base_directory"]["seen"], json!(["std 4→5"]));

        let mut current = project();
        layers().upgrade(&layers().current(), SavedPart::Project, &mut current).unwrap();
        assert_eq!(current, project(), "nothing to do");
        let mut part_way = project();
        layers()
            .upgrade(&formats(&[("core", 1), ("std", 4), ("build", 1)]), SavedPart::Project, &mut part_way)
            .unwrap();
        assert_eq!(part_way["base_directory"]["seen"], json!(["std 4→5"]), "only what's after");
        assert_eq!(part_way["graph"]["nodes"]["1"]["node"]["seen"], json!([]));
    }

    #[test]
    fn a_node_on_its_own_is_brought_up_to_date_the_same_way() {
        let mut node = json!({"type": "WriteLtx", "seen": []});
        layers().upgrade(&formats(&[("std", 3)]), SavedPart::Node("WriteLtx"), &mut node).unwrap();
        assert_eq!(node["seen"], json!(["std 3→4", "build WriteLtx"]));
    }

    #[test]
    fn what_is_too_old_or_too_new_is_refused_saying_so() {
        let mut value = project();
        let newer = layers().upgrade(&formats(&[("std", 6)]), SavedPart::Project, &mut value).unwrap_err();
        assert!(newer.contains("newer version") && newer.contains("std format 6"), "{newer}");
        let older = layers().upgrade(&formats(&[("std", 2)]), SavedPart::Project, &mut value).unwrap_err();
        assert!(older.contains("too old") && older.contains("std format 2"), "{older}");
        assert_eq!(value, project(), "untouched");
        layers().upgrade(&formats(&[("std", 3), ("another build", 9)]), SavedPart::Project, &mut value).unwrap();
    }
}
