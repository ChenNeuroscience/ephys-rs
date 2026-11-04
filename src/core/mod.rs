//! The core: the graph engine (`engine`), annotations (`annotation`), themes (`theme`), the log (`console`),
//! handing things from passes to windows (`exchange`), threads finishing in the background (`background`), how plots
//! over time lay out time (`time_axis`), quick actions (`quick_action`), and the GUI (`gui`).
//! None of it knows about particular node, edge or annotation types, or themes: the program supplies those
//! (see `examples/chen-neuro/`), declared with `edge_types!`, `node_set!` and `annotation_set!`, through `Components`.

pub mod annotation;
pub mod background;
pub mod base_directory;
pub mod console;
pub mod crash;
pub mod engine;
pub mod exchange;
pub mod gui;
pub mod layout;
pub mod quick_action;
pub mod theme;
pub mod time_axis;

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
    type Nodes: engine::NodeSet;
    type Annotations: annotation::AnnotationSet;
    /// The first is the default.
    const THEMES: &'static [theme::Theme];
    /// In the order they're offered.
    const QUICK_ACTIONS: &'static [quick_action::QuickAction<Self::Nodes>] = &[];
    /// Where recordings go (see `base_directory`): the first is the default.
    const BASE_DIRECTORIES: &'static [base_directory::BaseDirectoryType] = &[];

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
}
