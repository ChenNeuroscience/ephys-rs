//! ephys-rs: electrophysiology acquisition, processing and recording, built around a node graph.
//!
//! The crate's own modules are the framework: the graph engine (`engine`), annotations (`annotation`), themes
//! (`theme`), the log (`console`), triggers sent to passes (`pass_triggers`) and those that don't need a pass
//! (`triggers`), threads finishing in the background (`background`), how plots over time lay out time (`time_axis`),
//! quick actions (`quick_action`), and the GUI (`gui`). None of it knows about particular node, edge or annotation
//! types, or themes: those are in `standard` (the standard components), and a program's own. A program lists the parts
//! it's made of and runs them: see `Components`, `gui::run`, and `examples/chen-neuro/` for one. The kinds of data and
//! settings the standard components share (`Edges`, `DataStream`, `Setting`...) are declared in `standard`, and
//! re-exported here.

pub mod annotation;
pub mod background;
pub mod base_directory;
pub mod config_patches;
pub mod config_patching;
pub mod console;
pub mod crash;
pub mod engine;
pub mod global_settings;
pub mod gui;
pub mod layout;
pub mod pass_triggers;
pub mod quick_action;
pub mod search_folders;
pub mod standard;
pub mod theme;
pub mod time_axis;
pub mod triggers;

pub use standard::{Buffer, DataStream, Edges, Kind, Setting, SettingKind, Trigger, TriggerKind};

/// `version` ("0.12.0", as Cargo has it) as it's shown: "v0.12".
fn shown_version(version: &str) -> String {
    format!("v{}", version.strip_suffix(".0").unwrap_or(version))
}

/// The program's version, once it's running (see `full_version`).
static FULL_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The program's version: what `Components::version` says, "chen-neuro v0.900 (ephys-rs v0.12)", say. It can be asked for
/// from anywhere (for a writer's headers, or a crash report), as it's the same for the whole of a run. `version` is
/// the core's alone, for what wants only that. Until `gui::run` has started, it's just the core's.
pub fn full_version() -> String {
    #[cfg(test)]
    return <crate::components::ChenNeuro as Components>::version();
    #[cfg(not(test))]
    FULL_VERSION.get().cloned().unwrap_or_else(version)
}

/// Says what the program's version is, for `full_version`.
pub(crate) fn set_full_version(version: String) {
    let _ = FULL_VERSION.set(version);
}

/// The core's version, "ephys-rs v0.12": a count of the changes to the library (see docs/development.md). Tests show
/// "v0.y", so their snapshots don't change with it.
pub fn version() -> String {
    let version = if cfg!(test) { "v0.y".into() } else { shown_version(env!("CARGO_PKG_VERSION")) };
    format!("ephys-rs {version}")
}

/// The recommended version for a build (see `Components::version`): the lab's name, the build's version, then the
/// core's, "chen-neuro v0.900 (ephys-rs v0.12)". `version` is the build's, as Cargo has it:
/// `env!("CARGO_PKG_VERSION")`.
pub fn build_version(lab: &str, version: &str) -> String {
    let shown = if cfg!(test) { "v0.x".into() } else { shown_version(version) };
    format!("{lab} {shown} ({})", self::version())
}

/// What a program is built from: its node types (which bring their edge types), annotation types, themes, quick
/// actions and base directories.
pub trait Components: 'static {
    /// Its node types, from `nodes_enum!`.
    type Nodes: engine::NodesEnum;
    /// Its annotation types, from `annotations_enum!`.
    type Annotations: annotation::AnnotationsEnum;
    /// The first is the default.
    const THEMES: &'static [theme::Theme];
    /// In the order they're offered.
    const QUICK_ACTIONS: &'static [quick_action::QuickAction<Self::Nodes>] = &[];
    /// Where recordings go (see `base_directory`): the first is the default.
    const BASE_DIRECTORIES: &'static [base_directory::BaseDirectoryType] = &[];
    /// The config patches of the parts it's built from, in the order they're applied (see `config_patching`): the
    /// core's (`config_patches::PATCHES`), the standard components' (`standard::config_patches::PATCHES`),
    /// and its own, if it has node types of its own, say. Just the core's, unless it says.
    const CONFIG_PATCHES: &'static [config_patching::PatchSet] = &[config_patches::PATCHES];

    /// What's special about this build, in markdown, for the About window, under the version.
    const ABOUT: &'static str = "";
    /// Where the program's source is, for the Help menu and About window: this repository, unless it's a fork's.
    const SOURCE: &'static str = "https://github.com/ChenNeuroscience/ephys-rs";

    /// The program's version, shown in the status bar, given by `--version` (which is how CI names a release), and what
    /// `full_version` says, for nodes to keep in what they write. It's up to the build, but `build_version` is
    /// recommended: the core's alone doesn't say which build it is.
    fn version() -> String {
        version()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_shown_without_the_unused_part() {
        assert_eq!(shown_version("0.900.0"), "v0.900");
        assert_eq!(shown_version("1.2.3"), "v1.2.3", "if a build's using all of it");
        assert_eq!(build_version("chen-neuro", "0.900.0"), "chen-neuro v0.x (ephys-rs v0.y)", "as tests show it");
    }

    /// The framework knows nothing of the standard components, so a build can do without them: outside `standard`, and
    /// outside tests and comments, nothing names them, or the kinds of data and settings they declare.
    #[test]
    fn the_framework_does_not_use_the_standard_components() {
        const STANDARD: [&str; 9] = [
            "standard::",
            "crate::Edges",
            "crate::DataStream",
            "crate::Kind",
            "crate::Buffer",
            "crate::Setting",
            "crate::SettingKind",
            "crate::Trigger",
            "crate::TriggerKind",
        ];
        fn check(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy();
                if path.is_dir() {
                    if name != "standard" && name != "tests" {
                        check(&path, found);
                    }
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || name == "lib.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                // Up to its tests, which are of the whole program.
                let code = text.lines().take_while(|line| !line.starts_with("#[cfg(test)]"));
                for (number, line) in code.enumerate() {
                    let line = line.trim_start();
                    if !line.starts_with("//") && STANDARD.iter().any(|s| line.contains(s)) {
                        found.push(format!("{}:{}: {line}", path.display(), number + 1));
                    }
                }
            }
        }
        let mut found = Vec::new();
        check(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.is_empty(), "the framework uses the standard components:\n{}", found.join("\n"));
    }
}

// The tests are of the whole program: chen-neuro's.
#[cfg(test)]
extern crate self as ephys_rs;
#[cfg(test)]
#[path = "../examples/chen-neuro/src/components/mod.rs"]
mod components;
#[cfg(test)]
use components::{AnnotationKind, AnnotationTypes, NodeType, Nodes, example_graph};
