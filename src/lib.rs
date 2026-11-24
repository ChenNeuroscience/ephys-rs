//! ephys-rs: electrophysiology acquisition, processing and recording, built around a node graph.
//!
//! The crate's own modules are the framework: the graph engine (`engine`), annotations (`annotation`), themes
//! (`theme`), the log (`console`), commands between nodes (`commands`), threads finishing in the background (`background`), how
//! plots over time lay out time (`time_axis`), quick actions (`quick_action`), and the GUI (`gui`). None of it knows
//! about particular node, edge or annotation types, or themes: those are in the ephys-rs-standard crate (the standard
//! components, in standard/), and a program's own. A program lists the parts it's made of and runs them: see
//! `Components`, `gui::run`, and `examples/chen-neuro/` for one.

pub mod annotation;
pub mod background;
pub mod base_directory;
pub mod commands;
pub mod config_patches;
pub mod config_patching;
pub mod console;
pub mod crash;
pub mod engine;
pub mod global_settings;
pub mod gui;
pub mod layout;
pub mod quick_action;
pub mod search_folders;
pub mod theme;
pub mod time_axis;

/// The app shows a version ("0.12.0", as Cargo has it) as "v0.12".
fn shown_version(version: &str) -> String {
    format!("v{}", version.strip_suffix(".0").unwrap_or(version))
}

/// Once the program's running, its version's kept here (see `full_version`).
static FULL_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Code anywhere (a writer's headers, or a crash report) can ask for the program's version: what `Components::version`
/// says, "chen-neuro v0.900 (ephys-rs v0.12)", say. It's the same for the whole of a run. `version` is the core's
/// alone, for what wants only that. Until `gui::run` has started, this is just the core's.
pub fn full_version() -> String {
    #[cfg(test)]
    return <crate::components::ChenNeuro as Components>::version();
    #[cfg(not(test))]
    FULL_VERSION.get().cloned().unwrap_or_else(version)
}

/// As it starts, the app says what the program's version is, for `full_version`.
pub(crate) fn set_full_version(version: String) {
    let _ = FULL_VERSION.set(version);
}

/// The core's version, "ephys-rs v0.12", counts the changes to the library (see docs/development.md). Tests show
/// "v0.y", so their snapshots don't change with it.
pub fn version() -> String {
    let version = if cfg!(test) { "v0.y".into() } else { shown_version(env!("CARGO_PKG_VERSION")) };
    format!("ephys-rs {version}")
}

/// A build's version (see `Components::version`) is best made with this: the lab's name, the build's version, then the
/// core's, "chen-neuro v0.900 (ephys-rs v0.12)". `version` is the build's, as Cargo has it:
/// `env!("CARGO_PKG_VERSION")`.
pub fn build_version(lab: &str, version: &str) -> String {
    let shown = if cfg!(test) { "v0.x".into() } else { shown_version(version) };
    format!("{lab} {shown} ({})", self::version())
}

/// A program says what it's built from with this: its node types (which bring their edge types), annotation types,
/// themes, quick actions and base directories.
pub trait Components: 'static {
    /// The program's node types, from `nodes_enum!`.
    type Nodes: engine::NodesEnum;
    /// The program's annotation types, from `annotations_enum!`.
    type Annotations: annotation::AnnotationsEnum;
    /// The program's themes, the first being the default.
    const THEMES: &'static [theme::Theme];
    /// The basic view offers the program's quick actions in this order.
    const QUICK_ACTIONS: &'static [quick_action::QuickAction<Self::Nodes>] = &[];
    /// Recordings go in base directories of these types (see `base_directory`), the first being the default.
    const BASE_DIRECTORIES: &'static [base_directory::BaseDirectoryType] = &[];
    /// The app patches what it reads with the patches of the parts the program's built from, in this order (see
    /// `config_patching`): the core's (`config_patches::PATCHES`), the standard components'
    /// (`standard::config_patches::PATCHES`), and the program's own, if it has node types of its own, say. Just the
    /// core's, unless the program says.
    const CONFIG_PATCHES: &'static [config_patching::PatchSet] = &[config_patches::PATCHES];

    /// The About window says what's special about the build, in markdown, under the version.
    const ABOUT: &'static str = "";
    /// The Help menu and About window say where the program's source is: this repository, unless it's a fork's.
    const SOURCE: &'static str = "https://github.com/ChenNeuroscience/ephys-rs";

    /// The status bar shows the program's version, `--version` gives it (which is how CI names a release), and
    /// `full_version` says it, for nodes to keep in what they write. It's up to the build, but `build_version` is
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
}

// The tests are of the whole program, chen-neuro's, so they build its components and the standard components here, as
// it has them: `crate::components` and `crate::standard`, whose code reaches the framework as `ephys_rs`. (A
// dev-dependency on them would bring in a second copy of the framework, which the tests' couldn't be mixed with.)
#[cfg(test)]
extern crate self as ephys_rs;
#[cfg(test)]
#[path = "../examples/chen-neuro/src/components/mod.rs"]
mod components;
#[cfg(test)]
#[path = "../standard/src/lib.rs"]
#[allow(dead_code, reason = "what's public is for builds, not all of which the tests use")]
mod standard;
#[cfg(test)]
use components::{
    AnnotationKind, AnnotationTypes, Buffer, Command, DataStream, DataStreamKind, Edges, NodeType, Nodes, ValOut,
    example_graph,
};

/// A build only lists the edge types its nodes need. This build has two standard node types whose ports carry only
/// continuous data, so it lists continuous data alone, with no values or commands.
#[cfg(test)]
mod minimal_build {
    use crate::engine::{Graph, PortAddress, PortRole};
    use crate::standard::ephys::{Continuous, band_pass, downsample};

    ephys_rs::edge_types! {
        pub struct Edges;
        data_streams: pub enum DataStream, DataStreamKind, Buffer {
            Continuous(Continuous),
        }
        values: pub enum ValOut, ValueKind {}
        commands: pub enum Command, CommandKind {}
    }

    ephys_rs::nodes_enum! {
        edges: Edges;
        pub enum Nodes {
            BandPassFilter(band_pass::BandPassFilter),
            Downsample(downsample::Downsample),
        }
        pub enum NodeType;
    }

    #[test]
    fn a_build_needs_only_the_edge_types_its_nodes_ports_carry() {
        let mut graph = Graph::<Nodes>::default();
        let filter = graph.add(Nodes::BandPassFilter(Default::default()), [0.0, 0.0]);
        let downsample = graph.add(Nodes::Downsample(Default::default()), [0.0, 0.0]);
        let port = |node, role| PortAddress { node, role, port_idx: 0 };
        graph.connect(port(filter, PortRole::DataStreamOut), port(downsample, PortRole::DataStreamIn)).unwrap();
        let saved = serde_json::to_value(&graph).unwrap();
        assert_eq!(saved["edges"][0]["to"]["port"], "continuous");
    }
}
