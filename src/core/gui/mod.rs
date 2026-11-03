//! The main window: menus and a toolbar, then a dockable layout of panes (the graph canvas, node windows, and the
//! developer tools), then a status bar. Double-clicking a node opens its window: its own view, if it has one, otherwise its
//! settings (`node_window`). Node windows open docked, and can float.

mod actions;
mod arrange;
mod basic;
mod canvas;
mod create_menu;
mod dev_tools;
mod disk_space;
mod docking;
mod frame_times;
mod icons;
#[cfg(target_os = "macos")]
mod mac_menu;
mod node_tools;
mod node_window;
mod paste_settings;
mod style;
mod toolbar;

/// Pieces of the GUI for components to draw with, so they look of a piece with it.
pub mod widgets {
    pub use super::icons::Icon;
    pub use super::toolbar::toolbar_button;
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Id, Key, Modifiers, Pos2, RichText, ViewportCommand};
use serde::{Deserialize, Serialize};

use crate::core::Components;
use crate::core::annotation::{Annotation, AnnotationId, AnnotationSet, Annotations};
use crate::core::base_directory::{BaseDirectories, BaseDirectoryType, Place};
use crate::core::engine::{self, Graph, NodeId, NodeSet, Port, PortRef, Resolved, Runner, Snapshot, Stats};
use crate::core::layout::{Layout, Mode, Pane};
use crate::core::theme::{Colours, Theme};
use actions::Action;
use canvas::{Canvas, HEADER, NODE_WIDTH, Selection, Tool};
use create_menu::{Choice, CreateMenu};
use docking::Panes;

/// Runs the app built from `C`'s components, opening the file at `path` (or an empty graph).
pub fn run<C: Components>(path: Option<PathBuf>) -> eframe::Result {
    crate::core::console::init(true);
    crate::core::crash::install();
    crate::core::crash::test_if_asked();
    let mut app = App::<C::Nodes, C::Annotations>::open(None, C::THEMES)
        .with_quick_actions(C::QUICK_ACTIONS)
        .with_base_directories(C::BASE_DIRECTORIES);
    app.last_crash = crate::core::crash::take_latest();
    if let Some(report) = &app.last_crash {
        log::warn!("The last time ephys-rs ran, it crashed: a report is in {}", report.display());
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png")).expect("valid icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ephys-rs")
            .with_inner_size([1400.0, 850.0])
            .with_icon(icon),
        ..Default::default()
    };
    let result = eframe::run_native(
        "ephys-rs",
        options,
        Box::new(|cc| {
            let mut app = app;
            let saved: Option<String> = cc.storage.and_then(|s| eframe::get_value(s, THEME_KEY));
            if let Some(index) = saved.and_then(|name| app.themes.iter().position(|t| t.name == name)) {
                app.theme = index;
            }
            app.themes[app.theme].apply(&cc.egui_ctx);
            let saved: Option<serde_json::Value> = cc.storage.and_then(|s| eframe::get_value(s, BASE_DIRECTORY_KEY));
            app.base_directories = BaseDirectories::new(app.base_directories.types(), saved.as_ref());
            // After the preferences: what a file has comes first.
            if let Some(path) = path {
                app.open_path(path);
            }
            // For images in annotations.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            #[cfg(target_os = "macos")]
            {
                let checkable = |a| app.checked(a).is_some();
                app.mac_menu = mac_menu::MacMenu::install(&cc.egui_ctx, &checkable, C::Annotations::TYPES, C::THEMES);
            }
            Ok(Box::new(app))
        }),
    );
    // Writers finishing their files, say.
    crate::core::background::wait();
    result
}

/// The program's version, "v0.12": each commit counts the minor version up (see docs/development.md). Tests show
/// "v0.x", so their snapshots don't change with it.
pub fn version() -> String {
    if cfg!(test) {
        "v0.x".into()
    } else {
        format!("v{}.{}", env!("CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MINOR"))
    }
}

/// Where the theme (by name) is kept among the app's preferences (which eframe saves, separately from graph files).
const THEME_KEY: &str = "theme";
/// Where the base directories are kept with the app's preferences (see `BaseDirectories::save`).
const BASE_DIRECTORY_KEY: &str = "base directory";

/// Something a pane asked for, handled once the panes are drawn.
pub enum Event {
    /// Open the menu for adding a node, optionally connected to a port.
    OpenMenu {
        screen: Pos2,
        scene: Pos2,
        from: Option<Port>,
    },
    /// Open a node's window (see `Layout::toggle`), or close it: from a pane, if it's done from one.
    ToggleWindow(NodeId, Option<Pane>),
    /// Open a node's window (see `Layout::open`), or bring it to the front if it's open: from a pane, if it's done
    /// from one.
    OpenWindow(NodeId, Option<Pane>),
    FloatWindow(NodeId),
    Duplicate(NodeId),
    Disconnect(NodeId),
    Delete(NodeId),
    DeleteAnnotation(AnnotationId),
    Status(String),
    /// Do the quick action of this index.
    QuickAction(usize),
    /// Show the graph in the home pane.
    /// Do an action (from the graph's toolbar), with where a menu it opens goes.
    Perform(Action, Option<Pos2>),
    /// List a node in the basic view, or not.
    InBasic(NodeId, bool),
    /// Disable a node (or enable it), and the rest of the selection if it's in it.
    SetDisabled(NodeId, bool),
    SetEdgeDisabled(engine::Edge, bool),
    DeleteEdge(engine::Edge),
    /// Copy these nodes' settings.
    CopySettings(Vec<NodeId>),
    /// Paste settings from the clipboard: straight onto a node, if given and there's one node's settings there;
    /// otherwise through the window, to review them.
    PasteSettings(Option<NodeId>),
}

/// The file: the graph, its annotations, the window layout, and the base directories.
#[derive(Deserialize)]
#[serde(bound = "N: NodeSet, A: AnnotationSet")]
struct SaveFile<N, A> {
    graph: Graph<N>,
    #[serde(default)]
    annotations: Annotations<A>,
    #[serde(default)]
    layout: Option<Layout>,
    #[serde(default)]
    base_directory: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(bound = "N: NodeSet, A: AnnotationSet")]
struct SaveFileRef<'a, N, A> {
    graph: &'a Graph<N>,
    annotations: &'a Annotations<A>,
    layout: &'a Layout,
    base_directory: &'a serde_json::Value,
}

/// Where the settings are saved beside a recording in `place` that starts at `start`: named as the recording, if the
/// base directory names it (beside a writer's files of that name, or the folder it writes them in), otherwise when it
/// started.
fn settings_path(place: &Place, start: &chrono::DateTime<chrono::Local>) -> PathBuf {
    let name = place.name.clone().unwrap_or_else(|| start.format("%Y-%m-%d_%H%M%S").to_string());
    place.directory.join(format!("{name}{SETTINGS_EXTENSION}"))
}

/// What the settings saved with a recording are named with.
const SETTINGS_EXTENSION: &str = ".ephys-rs.json";

/// A message in the status bar.
struct Status {
    message: String,
    /// When to stop showing it (egui seconds).
    until: f64,
    /// Whether it's news of something done, rather than a problem.
    ok: bool,
}

pub struct App<N: NodeSet, A: AnnotationSet> {
    /// The file being edited, if it's been saved.
    path: Option<PathBuf>,
    /// The version in the file, if any: anything else has unsaved changes.
    saved: Option<u64>,
    /// The base directories as they are in the file (they aren't part of the graph's versions), if it's been saved or
    /// opened.
    saved_base_directory: Option<serde_json::Value>,
    /// The user has said to close despite unsaved changes.
    closing: bool,
    /// The themes to choose from, and the one in use.
    themes: &'static [Theme],
    theme: usize,
    /// Where the add-node menu opens from the toolbar.
    add_anchor: Option<Pos2>,
    #[cfg(target_os = "macos")]
    mac_menu: Option<mac_menu::MacMenu>,
    graph: Graph<N>,
    annotations: Annotations<A>,
    layout: Layout,
    canvas: Canvas,
    selection: Selection,
    menu: Option<CreateMenu>,
    /// The window for pasting settings, while it's open.
    paste: Option<paste_settings::PasteSettings<N>>,
    /// Shown in the status bar until the given time (egui seconds).
    status: Option<Status>,
    history: History<N, A>,
    /// The graph running, while it is.
    runner: Option<Runner<N>>,
    /// The acquisition clock when recording last started.
    recording_since: f64,
    /// While the nodes prepare to record, where the recording comes from when they're ready.
    preparing: Option<std::sync::mpsc::Receiver<engine::Recording>>,
    dev_tools: dev_tools::DevTools,
    /// Offered in the basic view.
    quick_actions: &'static [crate::core::quick_action::QuickAction<N>],
    /// Where recordings go: the one in use, in a section of the toolbar.
    base_directories: BaseDirectories,
    /// How full the disk the base directory is on is.
    disk_space: disk_space::DiskSpace,
    /// How many workers runs start with.
    workers: usize,
    /// How long each part of the GUI's taking, frame by frame, for the dev tools; and the last frame's CPU time.
    frame_times: frame_times::FrameTimes,
    last_cpu: Option<f32>,
    /// The snapshot the run last took, to mark what changes on the dev tools' chart.
    ran: Option<engine::Snapshot<N>>,
    /// The report of a crash the last time the program ran, until it's been said where it is.
    last_crash: Option<PathBuf>,
}

impl<N: NodeSet, A: AnnotationSet> App<N, A> {
    /// Opens the file at `path`, or an empty graph if there's no file there. `themes` mustn't be empty.
    pub fn open(path: Option<PathBuf>, themes: &'static [Theme]) -> Self {
        let annotations = Annotations::default();
        let graph = Graph::default();
        let mut app = App {
            path: None,
            saved: None,
            saved_base_directory: None,
            closing: false,
            themes,
            theme: 0,
            add_anchor: None,
            #[cfg(target_os = "macos")]
            mac_menu: None,
            history: History::new(&graph, &annotations),
            graph,
            annotations,
            layout: Layout::default(),
            canvas: Canvas::new(&themes[0].colours),
            selection: Selection::default(),
            menu: None,
            paste: None,
            status: None,
            runner: None,
            recording_since: 0.0,
            preparing: None,
            dev_tools: dev_tools::DevTools::default(),
            workers: engine::default_workers(),
            frame_times: Default::default(),
            last_cpu: None,
            ran: None,
            last_crash: None,
            quick_actions: &[],
            base_directories: BaseDirectories::new(&[], None),
            disk_space: Default::default(),
        };
        if let Some(path) = path {
            app.open_path(path);
        }
        app
    }

    /// Opens the file at `path`, or if there isn't one there, makes it the file to save to.
    fn open_path(&mut self, path: PathBuf) {
        if !path.exists() {
            // A new file, once saved.
            self.path = Some(path);
        } else if let Err(e) = self.load(&path) {
            let message = format!("Couldn't open {}: {e}", path.display());
            self.status = Some(Status { message, until: f64::INFINITY, ok: false });
        }
    }

    fn colours(&self) -> &'static Colours {
        let themes: &'static [Theme] = self.themes;
        &themes[self.theme].colours
    }

    /// The latest version of the graph, with all its configuration.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn snapshot(&self) -> Snapshot<N> {
        self.history.current.snapshot.clone()
    }

    /// Reports a problem in the status bar.
    fn set_status(&mut self, ctx: &egui::Context, message: impl Into<String>) {
        self.status = Some(Status { message: message.into(), until: ctx.input(|i| i.time) + 5.0, ok: false });
    }

    /// Reports something done in the status bar.
    fn set_done(&mut self, ctx: &egui::Context, message: impl Into<String>) {
        self.status = Some(Status { message: message.into(), until: ctx.input(|i| i.time) + 5.0, ok: true });
    }

    fn unsaved(&self) -> bool {
        let base_directory = self.saved_base_directory.as_ref();
        self.saved != Some(self.history.current.snapshot.version)
            || base_directory.is_some_and(|saved| *saved != self.base_directories.save())
    }

    fn file_name(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        name.unwrap_or_else(|| "Untitled".into())
    }

    fn file_dialog(&self) -> rfd::FileDialog {
        let dialog = rfd::FileDialog::new().add_filter("ephys-rs graph", &["json"]);
        match self.path.as_deref().and_then(Path::parent) {
            Some(dir) => dialog.set_directory(dir),
            None => dialog,
        }
    }

    /// Replaces everything with the file at `path`. Undo starts afresh.
    fn load(&mut self, path: &Path) -> Result<(), String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let file: SaveFile<N, A> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        // The mode stays as it is, opening a file (it starts basic, each time the app does).
        let mode = self.layout.mode;
        self.layout = file.layout.unwrap_or_default();
        self.layout.mode = mode;
        self.history = History::new(&file.graph, &file.annotations);
        self.graph = file.graph;
        self.annotations = file.annotations;
        if let Some(saved) = &file.base_directory {
            self.base_directories = BaseDirectories::new(self.base_directories.types(), Some(saved));
        }
        self.selection.clear();
        self.menu = None;
        self.canvas.frame_all();
        self.path = Some(path.to_owned());
        self.mark_saved();
        Ok(())
    }

    /// What there is now is what's in the file.
    fn mark_saved(&mut self) {
        self.saved = Some(self.history.current.snapshot.version);
        self.saved_base_directory = Some(self.base_directories.save());
    }

    /// Saves to the current file, or asks where if there isn't one. Returns whether it saved.
    fn save(&mut self, ctx: &egui::Context) -> bool {
        match self.path.clone() {
            Some(path) => self.save_to(ctx, path),
            None => self.save_as(ctx),
        }
    }

    fn save_as(&mut self, ctx: &egui::Context) -> bool {
        let dialog = self.file_dialog().set_file_name(format!("{}.json", self.file_name().trim_end_matches(".json")));
        match dialog.save_file() {
            Some(path) => self.save_to(ctx, path),
            None => false,
        }
    }

    /// What's written to a file.
    fn save_file<'a>(&'a self, base_directory: &'a serde_json::Value) -> SaveFileRef<'a, N, A> {
        SaveFileRef { graph: &self.graph, annotations: &self.annotations, layout: &self.layout, base_directory }
    }

    /// Saves the graph (as File › Save would, but without changing which file is open) beside a recording starting in
    /// `place` (see `settings_path`), so what recorded it is kept with it. On a thread of its own, so a slow disk
    /// doesn't hold up the GUI.
    fn save_with_recording(&self, place: &Place) {
        let path = settings_path(place, &chrono::Local::now());
        let base_directory = self.base_directories.save();
        let file = self.save_file(&base_directory);
        let json = match serde_json::to_string_pretty(&file) {
            Ok(json) => json,
            Err(e) => return log::error!("Couldn't save the settings with the recording: {e}"),
        };
        let write = move || {
            let written =
                path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, json));
            match written {
                Ok(()) => log::info!("Saved the settings with the recording, in {}", path.display()),
                Err(e) => log::error!("Couldn't save the settings with the recording, in {}: {e}", path.display()),
            }
        };
        match std::thread::Builder::new().name("saving settings with a recording".into()).spawn(write) {
            Ok(thread) => crate::core::background::finish_later(thread),
            Err(e) => log::error!("Couldn't save the settings with the recording: {e}"),
        }
    }

    fn save_to(&mut self, ctx: &egui::Context, path: PathBuf) -> bool {
        // Make sure the latest edits are in the version being marked as saved.
        self.history.commit(&self.graph, &self.annotations);
        let base_directory = self.base_directories.save();
        let file = self.save_file(&base_directory);
        let result = serde_json::to_string_pretty(&file)
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.set_done(ctx, format!("Saved {}", path.display()));
                self.path = Some(path);
                self.mark_saved();
                true
            }
            Err(e) => {
                self.set_status(ctx, format!("Couldn't save {}: {e}", path.display()));
                false
            }
        }
    }

    /// If there are unsaved changes, asks whether to save them first. Returns whether to carry on.
    fn confirm_discard(&mut self, ctx: &egui::Context, before: &str) -> bool {
        self.history.commit(&self.graph, &self.annotations);
        if !self.unsaved() {
            return true;
        }
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Unsaved changes")
            .set_description(format!("Save the changes to {} before {before}?", self.file_name()))
            .set_buttons(rfd::MessageButtons::YesNoCancel)
            .show();
        match answer {
            rfd::MessageDialogResult::Yes => self.save(ctx),
            rfd::MessageDialogResult::No => true,
            _ => false,
        }
    }

    fn new_file(&mut self, ctx: &egui::Context) {
        if !self.confirm_discard(ctx, "starting a new graph") {
            return;
        }
        self.graph = Graph::default();
        self.annotations = Annotations::default();
        self.history = History::new(&self.graph, &self.annotations);
        self.selection.clear();
        self.path = None;
        self.mark_saved();
    }

    fn open_file(&mut self, ctx: &egui::Context) {
        if !self.confirm_discard(ctx, "opening another") {
            return;
        }
        if let Some(path) = self.file_dialog().pick_file()
            && let Err(e) = self.load(&path)
        {
            self.set_status(ctx, format!("Couldn't open {}: {e}", path.display()));
        }
    }

    /// Whether settings are locked: while recording, or preparing to.
    fn locked(&self) -> bool {
        self.history.current.snapshot.recording.is_some() || self.preparing.is_some()
    }

    /// While recording, undoes any change to how the graph processes (a setting, an edge, a node added or
    /// removed), keeping where nodes are and what they're called.
    fn lock(&mut self, ctx: &egui::Context) {
        let recorded = &self.history.current.snapshot.graph;
        if !self.locked() || engine::same_processing(&self.graph, recorded) {
            return;
        }
        let mut graph = (**recorded).clone();
        for (id, entry) in self.graph.iter() {
            if let Some(kept) = graph.get_mut(id) {
                (kept.pos, kept.label) = (entry.pos, entry.label.clone());
            }
        }
        self.graph = graph;
        self.set_status(ctx, "Settings are locked while recording");
    }

    /// Starts running whatever's latest, including edits not yet taken as a version.
    fn play(&mut self) {
        self.history.commit(&self.graph, &self.annotations);
        self.runner = Some(Runner::start(&self.history.current.snapshot, self.workers));
        self.frame_times.run_started = Some(self.frame_times.now());
        self.frame_times.run_restarted();
        self.ran = Some(self.history.current.snapshot.clone());
    }

    /// Has the nodes prepare to record (see `Node::prepare_to_record`), on a thread of their own: once they're
    /// ready, recording starts (in `keep_running`).
    fn prepare_to_record(&mut self, ctx: &egui::Context) {
        self.history.commit(&self.graph, &self.annotations);
        let snapshot = self.history.current.snapshot.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        let base = self.base_directories.place();
        match &base {
            Some(base) => log::info!("Preparing to record, in {}", base),
            None => log::info!("Preparing to record"),
        }
        let spawned = std::thread::Builder::new().name("preparing to record".into()).spawn(move || {
            let _ = sender.send(engine::Recording::prepare(&snapshot, base));
            repaint.request_repaint();
        });
        match spawned {
            Ok(_) => self.preparing = Some(receiver),
            Err(e) => self.set_status(ctx, format!("Couldn't prepare to record: {e}")),
        }
    }

    /// Starts or stops recording: a new snapshot, which the run carries on with (see `Snapshot::with_recording`).
    /// Recording isn't an edit, so it's not undone, and doesn't count as an unsaved change.
    fn set_recording(&mut self, recording: Option<Arc<engine::Recording>>) {
        let on = recording.is_some();
        let place = recording.as_ref().and_then(|r| r.base_directory().cloned());
        let Some((old, new)) = self.history.set_recording(recording) else { return };
        if self.saved == Some(old) {
            self.saved = Some(new);
        }
        let clock = self.runner.as_ref().map_or(0.0, |r| r.clock());
        if let Some(base) = self.base_directories.current() {
            match &place {
                Some(place) => base.recording_started(place),
                None if !on => base.recording_stopped(),
                None => {}
            }
        }
        if on {
            self.recording_since = clock;
            log::info!("Recording, from {}", clock_text(clock));
        } else {
            log::info!("Stopped recording after {}", clock_text(clock - self.recording_since));
        }
    }

    fn undo(&mut self) {
        if let Some(version) = self.history.undo() {
            self.restore(version);
        }
    }

    fn redo(&mut self) {
        if let Some(version) = self.history.redo() {
            self.restore(version);
        }
    }

    fn restore(&mut self, (graph, annotations): (Graph<N>, Annotations<A>)) {
        self.graph = graph;
        self.annotations = annotations;
        self.selection.retain_existing(&self.graph, &self.annotations);
        self.menu = None;
    }

    fn delete_selection(&mut self) {
        for id in std::mem::take(&mut self.selection.nodes) {
            self.graph.remove(id);
        }
        for id in std::mem::take(&mut self.selection.annotations) {
            self.annotations.remove(id);
        }
        if let Some(edge) = self.selection.edge.take() {
            self.graph.disconnect(edge);
        }
    }

    /// Copies the selection, and the edges between the selected nodes, a little down and to the right.
    fn duplicate_selection(&mut self) {
        let offset = |pos: [f32; 2]| [pos[0] + 30.0, pos[1] + 30.0];
        let mut copies = HashMap::new();
        for id in &self.selection.nodes {
            let Some(entry) = self.graph.get(*id) else { continue };
            let (node, pos) = (entry.node.clone(), offset(entry.pos));
            copies.insert(*id, self.graph.add(node, pos));
        }
        for edge in self.graph.edges().to_vec() {
            if let (Some(from), Some(to)) = (copies.get(&edge.from.node), copies.get(&edge.to.node)) {
                let mut copy = edge;
                copy.from = PortRef { node: *from, ..edge.from };
                copy.to = PortRef { node: *to, ..edge.to };
                let _ = self.graph.connect(copy.source(), copy.target());
            }
        }
        let notes: Vec<Annotation<A>> =
            self.selection.annotations.iter().filter_map(|id| self.annotations.get(*id).cloned()).collect();
        self.selection.clear();
        self.selection.nodes.extend(copies.values());
        for note in notes {
            let id = self.annotations.add(Annotation { pos: offset(note.pos), ..note });
            self.selection.annotations.insert(id);
        }
    }

    fn select_all(&mut self) {
        self.selection.nodes = self.graph.ids().collect();
        self.selection.annotations = self.annotations.ids().collect();
    }

    /// Offers `quick_actions` in the basic view.
    /// With `types` of base directory to choose from (the first in use, until another's chosen).
    pub fn with_base_directories(mut self, types: &'static [BaseDirectoryType]) -> Self {
        self.base_directories = BaseDirectories::new(types, None);
        self
    }

    pub fn with_quick_actions(mut self, quick_actions: &'static [crate::core::quick_action::QuickAction<N>]) -> Self {
        self.quick_actions = quick_actions;
        self
    }

    /// `id` and the rest of the selection, if it's in it; otherwise just `id`.
    fn selected_or(&self, id: NodeId) -> Vec<NodeId> {
        if self.selection.nodes.contains(&id) {
            let mut ids: Vec<NodeId> = self.selection.nodes.iter().copied().collect();
            ids.sort();
            ids
        } else {
            vec![id]
        }
    }

    /// Copies nodes' settings to the clipboard.
    fn copy_settings(&mut self, ctx: &egui::Context, ids: &[NodeId]) {
        if ids.is_empty() {
            return;
        }
        ctx.copy_text(paste_settings::copy(&self.graph, ids));
        let what = match ids {
            [id] => self.graph.get(*id).map_or("a node".into(), |e| e.label.clone()),
            _ => format!("{} nodes", ids.len()),
        };
        self.set_done(ctx, format!("Copied the settings of {what}"));
    }

    /// Where settings are pasted straight onto: the node selected, if just one is.
    fn paste_target(&self) -> Option<NodeId> {
        (self.selection.nodes.len() == 1).then(|| self.selection.nodes.iter().next().copied()).flatten()
    }

    /// Settings pasted the usual way (Ctrl/⌘ V, while nothing's being typed in) are pasted as settings, wherever the
    /// pointer is, rather than reaching the canvas as text.
    fn paste_settings_pasted(&mut self, ctx: &egui::Context) {
        let pasted = |e: &egui::Event| matches!(e, egui::Event::Paste(text) if paste_settings::is_settings(text));
        let text = ctx.input_mut(|i| match i.events.iter().position(pasted).map(|at| i.events.remove(at)) {
            Some(egui::Event::Paste(text)) => Some(text),
            _ => None,
        });
        if let Some(text) = text {
            self.paste_text(ctx, &text, self.paste_target());
        }
    }

    /// Pastes settings from the clipboard: straight onto `target`, if given and there's just one node's settings
    /// there; otherwise opening the window to review them.
    fn open_paste(&mut self, ctx: &egui::Context, target: Option<NodeId>) {
        let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
        self.paste_text(ctx, &text, target);
    }

    fn paste_text(&mut self, ctx: &egui::Context, text: &str, target: Option<NodeId>) {
        let paste = match paste_settings::PasteSettings::new(text, &self.graph) {
            Ok(paste) => paste,
            Err(problem) => return self.set_status(ctx, problem),
        };
        match target.and_then(|target| paste.onto(target, &self.graph)) {
            Some(Ok(pasted)) if !self.locked() => self.paste_settings(ctx, vec![pasted], Vec::new()),
            Some(Ok(_)) => self.set_status(ctx, "Settings are locked while recording"),
            Some(Err(problem)) => {
                log::warn!("Didn't paste settings: {problem}");
                self.set_status(ctx, problem);
            }
            None => self.paste = Some(paste),
        }
    }

    /// Pastes settings onto nodes, logging what changed, and what was left out.
    fn paste_settings(&mut self, ctx: &egui::Context, pasted: Vec<paste_settings::Pasted<N>>, skipped: Vec<String>) {
        let count = pasted.len();
        for paste_settings::Pasted { target, node, from } in pasted {
            let Some(entry) = self.graph.get_mut(target) else { continue };
            let changes = paste_settings::changed_settings(&entry.node, &node);
            log::info!("Pasted the settings of {from} onto {}: changing {changes}", entry.label);
            entry.node = node;
        }
        for reason in &skipped {
            log::warn!("Didn't paste the settings of {reason}");
        }
        let left_out = if skipped.is_empty() { String::new() } else { format!(", leaving out {}", skipped.len()) };
        let plural = if count == 1 { "" } else { "s" };
        self.set_done(ctx, format!("Pasted settings onto {count} node{plural}{left_out} (see the console)"));
    }

    /// Applies settings the passes have proposed (see `Pass::propose`): an auto-detected gain, say. Not while
    /// recording, when settings are locked.
    fn apply_proposals(&mut self) {
        let Some(runner) = &self.runner else { return };
        let proposals = runner.exchange().take_proposals();
        let locked = self.locked();
        for (id, setting, value) in proposals {
            let Ok(value) = value.downcast::<<N::Edges as engine::Edges>::Setting>() else { continue };
            let Some(entry) = self.graph.get_mut(id) else { continue };
            let name = entry.node.settings().get(setting).map_or("a setting", |s| s.name);
            if locked {
                log::warn!("{}'s {name} weren't changed: settings are locked while recording", entry.label);
                continue;
            }
            entry.node.set_setting(setting, *value);
            log::info!("{}'s {name} set from its data", entry.label);
        }
    }

    /// Says where the report of the last time's crash is, until it's dismissed.
    fn crash_window(&mut self, ctx: &egui::Context) {
        let Some(report) = &self.last_crash else { return };
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("last crash")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.heading("ephys-rs crashed last time");
            ui.label("A report of what happened, to send to whoever looks after it, is in:");
            ui.add(egui::Label::new(egui::RichText::new(report.display().to_string()).monospace()).selectable(true));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Show the report").clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(crate::core::crash::file_url(report)));
                }
                if ui.button("Copy where it is").clicked() {
                    ctx.copy_text(report.display().to_string());
                }
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.last_crash = None;
        }
    }

    /// The window for pasting settings, while it's open; pasting them when it says to.
    fn paste_window(&mut self, ctx: &egui::Context) {
        let (colours, locked) = (self.colours(), self.locked());
        let Some(paste) = &mut self.paste else { return };
        match paste.show(ctx, &self.graph, colours, locked) {
            paste_settings::Outcome::Open => {}
            paste_settings::Outcome::Cancelled => self.paste = None,
            paste_settings::Outcome::Paste(pasted, skipped) => {
                self.paste = None;
                self.paste_settings(ctx, pasted, skipped);
            }
        }
    }

    /// Shows the basic view or the graph in the home pane, bringing it to the front.
    fn show_mode(&mut self, mode: Mode) {
        self.layout.mode = mode;
        self.layout.show(Pane::Home);
    }

    /// The quick actions offered, by index: those that apply to the graph and layout as they are.
    fn offered(&self) -> Vec<usize> {
        let state = crate::core::quick_action::QuickState { graph: &self.graph, layout: &self.layout };
        (0..self.quick_actions.len()).filter(|&i| (self.quick_actions[i].run)(&state).is_some()).collect()
    }

    /// Does a quick action: changes the graph, and opens windows, as it says (unless it no longer applies).
    fn quick_action(&mut self, index: usize) {
        let Some(action) = self.quick_actions.get(index) else { return };
        if self.locked() {
            return;
        }
        let state = crate::core::quick_action::QuickState { graph: &self.graph, layout: &self.layout };
        let Some(change) = (action.run)(&state) else { return };
        self.graph = change.graph;
        self.layout = change.layout;
        if change.arrange {
            for (id, pos) in arrange::arrange(&self.graph) {
                if let Some(entry) = self.graph.get_mut(id) {
                    entry.pos = pos;
                }
            }
            self.canvas.frame_all();
        }
        let recording = self.history.current.snapshot.recording.is_some() || self.preparing.is_some();
        let types = self.base_directories.types();
        if let (Some(name), false) = (change.base_directory, recording)
            && let Some(index) = types.iter().position(|t| t.name == name)
        {
            self.base_directories.choose(index);
        }
        log::info!("Quick action \"{}\": {}", action.name, change.summary);
    }

    fn perform(&mut self, ctx: &egui::Context, action: Action) {
        // What's done on the graph shows it.
        if matches!(
            action,
            Action::AddNode | Action::DrawTool(_) | Action::FrameAll | Action::ActualSize | Action::Arrange
        ) {
            self.show_mode(Mode::Advanced);
        }
        match action {
            Action::New => self.new_file(ctx),
            Action::Open => self.open_file(ctx),
            Action::Save => {
                self.save(ctx);
            }
            Action::SaveAs => {
                self.save_as(ctx);
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Duplicate => self.duplicate_selection(),
            Action::Delete => self.delete_selection(),
            Action::SelectAll => self.select_all(),
            Action::CopySettings => {
                let mut ids: Vec<NodeId> = self.selection.nodes.iter().copied().collect();
                ids.sort();
                self.copy_settings(ctx, &ids);
            }
            Action::PasteSettings => self.open_paste(ctx, self.paste_target()),
            Action::ToggleDisabled => {
                let all = self.selection.nodes.iter().all(|id| self.graph.get(*id).is_some_and(|e| e.disabled));
                for id in self.selection.nodes.clone() {
                    if let Some(entry) = self.graph.get_mut(id) {
                        entry.disabled = !all;
                    }
                }
                if let Some(edge) = self.selection.edge {
                    let disabled = self.graph.is_edge_disabled(&edge);
                    self.graph.set_edge_disabled(edge, !disabled);
                }
            }
            Action::Home => self.layout.show(Pane::Home),
            Action::Advanced => {
                let mode = if self.layout.mode == Mode::Advanced { Mode::Basic } else { Mode::Advanced };
                self.show_mode(mode);
            }
            Action::CloseHome => self.layout.hide(Pane::Home),
            Action::Arrange => {
                for (id, pos) in arrange::arrange(&self.graph) {
                    if let Some(entry) = self.graph.get_mut(id) {
                        entry.pos = pos;
                    }
                }
                self.canvas.frame_all();
            }
            Action::FrameAll => self.canvas.frame_all(),
            Action::ActualSize => self.canvas.reset_zoom(),
            Action::ResetLayout => {
                let mode = self.layout.mode;
                self.layout = Layout::default();
                self.layout.mode = mode;
            }
            // Goes through `confirm_close`, like closing the window.
            Action::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            Action::AddNode => {
                let at = self.canvas.view_centre();
                let screen = self.add_anchor.take().unwrap_or_else(|| self.canvas.to_screen(at));
                self.menu = Some(CreateMenu::new(screen, at, None));
            }
            Action::ToggleDevTools => self.layout.toggle_dev_tools(),
            Action::Play => {
                if self.runner.is_some() {
                    self.set_recording(None);
                    self.preparing = None;
                    self.runner = None;
                } else {
                    self.play();
                }
            }
            Action::Record => {
                if self.history.current.snapshot.recording.is_some() {
                    self.set_recording(None);
                } else if self.preparing.take().is_some() {
                    log::info!("Cancelled recording");
                } else {
                    if self.runner.is_none() {
                        self.play();
                    }
                    self.prepare_to_record(ctx);
                }
            }
            Action::Theme(index) => {
                self.theme = index;
                self.themes[index].apply(ctx);
            }
            Action::DrawTool(index) if !A::is_drawn(index) => {
                self.canvas.insert_chosen(index, &mut self.annotations, &mut self.selection);
            }
            Action::DrawTool(index) => {
                let tool = Tool::Draw(index);
                self.canvas.tool = if self.canvas.tool == tool { Tool::Select } else { tool };
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input() || self.menu.is_some();
        for action in Action::pressed(ctx, typing) {
            self.perform(ctx, action);
        }
        if ctx.egui_wants_keyboard_input() || self.menu.is_some() {
            return;
        }
        self.paste_settings_pasted(ctx);
        let key = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if key(Key::Enter) {
            if let Some(id) = self.selection.single() {
                self.layout.toggle(id, None);
            } else if let (Some(&id), true) = (self.selection.annotations.first(), self.selection.nodes.is_empty()) {
                self.canvas.edit_annotation(id);
            }
        }
        if key(Key::Escape) {
            self.selection.clear();
            self.canvas.tool = Tool::Select;
        }
        if key(Key::Tab)
            && let Some((screen, scene)) = self.canvas.pointer(ctx)
        {
            self.menu = Some(CreateMenu::new(screen, scene, None));
        }
    }

    /// Keeps the runner on the latest version of the graph (see `Runner::update`).
    fn keep_running(&mut self, ctx: &egui::Context) {
        if let Some(recording) = self.preparing.as_ref().and_then(|r| r.try_recv().ok()) {
            self.preparing = None;
            if let Some(place) = recording.base_directory() {
                self.save_with_recording(place);
            }
            self.set_recording(Some(Arc::new(recording)));
        }
        let Some(runner) = &self.runner else { return };
        let snapshot = &self.history.current.snapshot;
        let same_epoch = runner.update(snapshot);
        // Marking what changed, for the dev tools' chart.
        if let Some(before) = self.ran.take_if(|ran| ran.version != snapshot.version) {
            if let Some((label, detail)) = describe_change(&before, snapshot) {
                use frame_times::MarkKind;
                let kind = if before.recording.is_some() != snapshot.recording.is_some() {
                    MarkKind::Recording
                } else if same_epoch {
                    MarkKind::Version
                } else {
                    MarkKind::Epoch
                };
                let label = match kind {
                    MarkKind::Recording => label,
                    MarkKind::Epoch => format!("v{} new epoch", snapshot.version),
                    MarkKind::Version => format!("v{}", snapshot.version),
                };
                self.frame_times.mark(snapshot.version, kind, label, detail);
            }
            self.ran = Some(snapshot.clone());
        }
        for first in runner.first_uses() {
            self.frame_times.first_use(first.version, first.sequence, first.at);
        }
        // Keep the stats in the status bar fresh.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    /// Returns whether the run's stats were clicked (to open the dev tools).
    fn status_bar(&self, ui: &mut egui::Ui) -> bool {
        let now = ui.input(|i| i.time);
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(version()).on_hover_text("The version: each change to the program counts it up");
                if let Some(runner) = &self.runner {
                    ui.separator();
                    let stats = ui.scope(|ui| self.run_stats(ui, &runner.stats())).response.rect;
                    // Watched rather than made a button, so the stats keep their own tooltips.
                    if ui.rect_contains_pointer(stats) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        clicked = ui.input(|i| i.pointer.primary_clicked());
                    }
                }
            });
            match (&self.status, self.canvas.hint()) {
                (Some(status), _) if now < status.until => {
                    let colours = self.colours();
                    let colour = if status.ok { colours.success } else { colours.warning };
                    ui.label(RichText::new(&status.message).color(colour))
                }
                (_, Some(hint)) if self.layout.mode == Mode::Advanced => ui.label(hint),
                _ if !self.layout.is_shown(Pane::Home) => {
                    ui.weak("Home, at the left of the toolbar, brings back the basic view or the graph")
                }
                _ if self.layout.mode == Mode::Basic => ui.weak(
                    "Open a component, or add some with a quick action · the graph shows how it all fits together",
                ),
                _ => ui
                    .weak("Double-click, right-click or Tab on the canvas to add a node · drag from a port to connect"),
            };
        });
        clicked
    }

    /// How the run is going: workers, passes and their timing, and memory.
    fn run_stats(&self, ui: &mut egui::Ui, stats: &Stats) {
        let colours = self.colours();
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        // Right to left.
        if stats.dropped > 0 {
            let dropped = ui.label(RichText::new(format!("{} dropped", stats.dropped)).color(colours.warning));
            dropped.on_hover_text("Passes skipped because the workers couldn't keep up");
            ui.separator();
        }
        ui.label(format!("{:.1} MB", stats.memory as f64 / 1e6)).on_hover_text("Memory for the workers' buffers");
        ui.separator();
        ui.label(format!("lag {:.1} ms", ms(stats.max_lag)))
            .on_hover_text("Longest time in the last second from a pass's data being due to its being processed");
        ui.separator();
        ui.label(format!("pass {:.2} ms avg, {:.2} max", ms(stats.mean_time), ms(stats.max_time)))
            .on_hover_text("How long passes took to run, over the last second");
        ui.separator();
        ui.label(format!("{} passes/s", stats.passes_per_second));
        ui.separator();
        ui.label(format!("{} workers ({} busy)", stats.workers, stats.busy));
        ui.label(RichText::new("Running").color(colours.success)).on_hover_text(format!(
            "{} passes run, of {:.0} ms each",
            stats.completed,
            engine::PASS_DURATION * 1000.0
        ));
    }

    /// Floating node windows, over everything else. They're styled like docked ones: a tab bar with the one
    /// tab, rather than a window title bar.
    fn floating_windows(&mut self, ctx: &egui::Context, resolved: &Resolved<N::Edges>, events: &mut Vec<Event>) {
        let locked = self.locked();
        let colours = self.colours();
        let advanced = self.layout.mode == Mode::Advanced;
        let base = self.base_directories.place();
        for id in self.layout.floating() {
            let started = Instant::now();
            let Some(entry) = self.graph.get(id) else { continue };
            let has_view = entry.node.has_view();
            let beside = self.canvas.to_screen(egui::pos2(entry.pos[0] + NODE_WIDTH + 20.0, entry.pos[1]));
            let title = entry.label.clone();
            let colour = style::category_colour(entry.node.info().category);
            let mut clicks = docking::HeaderClicks::default();
            let response = egui::Window::new(&title)
                .id(Id::unique(("node-window", id)))
                .title_bar(false)
                .collapsible(false)
                .frame(egui::Frame::window(&ctx.global_style()).inner_margin(0).corner_radius(2))
                .default_pos(beside)
                .default_size(if has_view { [820.0, 520.0] } else { [340.0, 460.0] })
                .resizable(true)
                .show(ctx, |ui| {
                    clicks = docking::floating_header(ui, &title, colour);
                    egui::Frame::new().inner_margin(8).show(ui, |ui| {
                        docking::node_ui(
                            ui,
                            &mut self.graph,
                            resolved,
                            colours,
                            self.runner.as_ref(),
                            base.as_ref(),
                            locked,
                            advanced,
                            id,
                            events,
                        );
                    });
                });
            if let Some(response) = response
                && (response.response.clicked() || response.response.drag_started())
            {
                self.selection.only(id);
            }
            if clicks.close {
                events.push(Event::ToggleWindow(id, None));
            }
            if clicks.dock {
                self.layout.dock(id, None);
            }
            // Timed as the window, as it is when docked.
            self.frame_times.add(&title, ms_since(started));
        }
    }

    fn handle(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::OpenMenu { screen, scene, from } => self.menu = Some(CreateMenu::new(screen, scene, from)),
            Event::ToggleWindow(id, from) => self.layout.toggle(id, from),
            Event::OpenWindow(id, from) => self.layout.open(id, from),
            Event::FloatWindow(id) => self.layout.float(id),
            Event::Duplicate(id) => {
                if !self.selection.nodes.contains(&id) {
                    self.selection.only(id);
                }
                self.duplicate_selection();
            }
            Event::Disconnect(id) => {
                for edge in self.graph.edges().to_vec() {
                    if edge.from.node == id || edge.to.node == id {
                        self.graph.disconnect(edge);
                    }
                }
            }
            Event::Delete(id) => {
                if !self.selection.nodes.contains(&id) {
                    self.selection.only(id);
                }
                self.delete_selection();
            }
            Event::DeleteAnnotation(id) => {
                self.annotations.remove(id);
                self.selection.annotations.remove(&id);
            }
            Event::Status(message) => self.set_status(ctx, message),
            Event::QuickAction(index) => self.quick_action(index),
            Event::Perform(action, anchor) => {
                if anchor.is_some() {
                    self.add_anchor = anchor;
                }
                self.perform(ctx, action);
            }
            Event::SetDisabled(id, disabled) => {
                for id in self.selected_or(id) {
                    if let Some(entry) = self.graph.get_mut(id) {
                        entry.disabled = disabled;
                    }
                }
            }
            Event::SetEdgeDisabled(edge, disabled) => self.graph.set_edge_disabled(edge, disabled),
            Event::DeleteEdge(edge) => self.graph.disconnect(edge),
            Event::CopySettings(ids) => self.copy_settings(ctx, &ids),
            Event::PasteSettings(target) => self.open_paste(ctx, target),
            Event::InBasic(id, shown) => {
                if let Some(entry) = self.graph.get_mut(id) {
                    entry.basic = Some(shown);
                }
            }
        }
    }

    fn create_menu(&mut self, ctx: &egui::Context, resolved: &Resolved<N::Edges>) {
        let Some(menu) = &mut self.menu else { return };
        let (choice, keep) = menu.show(ctx, &self.graph, resolved);
        let (scene, from) = (menu.scene, menu.from);
        if !keep {
            self.menu = None;
        }
        let connect = |app: &mut Self, from: Port, to: Port| {
            if let Err(e) = app.graph.connect(from, to) {
                app.set_status(ctx, format!("Can't connect: {e}"));
            }
        };
        match choice {
            Some(Choice::New(type_index)) => {
                // Beside the port it's connected to, on the side its edge leaves from.
                let pos = match from {
                    Some(port) if canvas::on_right(port.role) => [scene.x + 40.0, scene.y - HEADER],
                    Some(_) => [scene.x - NODE_WIDTH - 40.0, scene.y - HEADER],
                    None => [scene.x - NODE_WIDTH / 2.0, scene.y - HEADER / 2.0],
                };
                let id = self.graph.add(N::create(type_index), pos);
                self.selection.only(id);
                if let Some(from) = from
                    && let Some(port) = create_menu::port_on_new::<N>(from, resolved, type_index)
                {
                    connect(self, from, Port { node: id, ..port });
                }
            }
            Some(Choice::Existing(port)) => {
                if let Some(from) = from {
                    connect(self, from, port);
                }
            }
            None => {}
        }
    }

    /// Whether the menus are in the system's menu bar (on macOS), rather than in the window.
    fn native_menu(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.mac_menu.is_some();
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Asks before closing with unsaved changes.
    fn confirm_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if self.confirm_discard(ctx, "closing") {
                self.closing = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.frame_times.begin(self.last_cpu.take());
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        // Keep (and echo) what's been logged, so the log's queue doesn't fill.
        crate::core::console::drain();
        self.shortcuts(&ctx);
        #[cfg(target_os = "macos")]
        for action in self.mac_menu.as_ref().map(|m| m.actions()).unwrap_or_default() {
            self.perform(&ctx, action);
        }
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.mac_menu {
            menu.sync(|a| self.checked(a) == Some(true));
        }
        self.graph.prune_edges();
        self.selection.retain_existing(&self.graph, &self.annotations);
        self.layout.retain_nodes(&self.graph);
        self.layout.show_annotation_pane(!self.selection.annotations.is_empty());
        let resolved = self.graph.resolve();
        let open_windows = self.layout.open_windows();
        self.canvas.colours = self.colours();
        self.frame_times.add("Upkeep", ms_since(started));

        let started = Instant::now();
        self.confirm_close(&ctx);
        let mut chosen = None;
        if !self.native_menu() {
            egui::Panel::top("menu_bar").show(ui, |ui| chosen = self.menu_bar(ui));
        }
        egui::Panel::top("toolbar").show(ui, |ui| chosen = chosen.or(self.toolbar(ui)));
        if let Some(action) = chosen {
            self.perform(&ctx, action);
        }
        self.frame_times.add("Menus and toolbar", ms_since(started));
        let started = Instant::now();
        let stats_clicked = egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui)).inner;
        self.frame_times.add("Status bar", ms_since(started));
        if stats_clicked && !self.layout.is_shown(Pane::DevTools) {
            self.layout.toggle_dev_tools();
        }
        let mut events = Vec::new();
        // Panes don't paint their own backgrounds, so this shows behind them.
        let colours = self.colours();
        let background = egui::Frame::NONE.fill(colours.surface);
        let locked = self.locked();
        let mut timings = Vec::new();
        let graph_tools = self.graph_tools();
        let base = self.base_directories.place();
        let offered = self.offered();
        egui::CentralPanel::default().frame(background).show(ui, |ui| {
            let mut panes = Panes {
                graph: &mut self.graph,
                annotations: &mut self.annotations,
                resolved: &resolved,
                colours,
                runner: self.runner.as_ref(),
                base_directory: base.as_ref(),
                snapshot: &self.history.current.snapshot,
                mode: self.layout.mode,
                graph_tools,
                quick_actions: self.quick_actions,
                offered: &offered,
                workers: self.workers,
                locked,
                dev_tools: &mut self.dev_tools,
                canvas: &mut self.canvas,
                selection: &mut self.selection,
                open_windows: &open_windows,
                events: &mut events,
                frame_times: &self.frame_times,
                timings: &mut timings,
            };
            self.layout.ui(ui, &mut panes);
        });
        for (part, ms) in timings {
            self.frame_times.add(&part, ms);
        }
        self.floating_windows(&ctx, &resolved, &mut events);
        let started = Instant::now();
        for event in events {
            self.handle(&ctx, event);
        }
        self.create_menu(&ctx, &resolved);
        self.paste_window(&ctx);
        self.crash_window(&ctx);
        self.apply_proposals();
        self.lock(&ctx);
        self.history.record(&self.graph, &self.annotations, &ctx);
        self.keep_running(&ctx);
        self.frame_times.add("Upkeep", ms_since(started));
    }
}

/// What changed from one snapshot to the next, as it runs: a short label, and the detail; nothing if only how it's
/// shown changed (nodes moved, say).
fn describe_change<N: NodeSet>(before: &engine::Snapshot<N>, after: &engine::Snapshot<N>) -> Option<(String, String)> {
    let mut changes = Vec::new();
    let mut label = "changed".to_owned();
    match (before.recording.is_some(), after.recording.is_some()) {
        (false, true) => {
            changes.push("Recording started".to_owned());
            label = "recording".into();
        }
        (true, false) => {
            changes.push("Recording stopped".to_owned());
            label = "stopped recording".into();
        }
        _ => {}
    }
    let (old, new) = (&before.graph, &after.graph);
    let names = |graph: &Graph<N>, ids: Vec<NodeId>| -> String {
        ids.iter().filter_map(|id| graph.get(*id).map(|e| e.label.clone())).collect::<Vec<_>>().join(", ")
    };
    let added: Vec<NodeId> = new.iter().map(|(id, _)| id).filter(|id| !old.contains(*id)).collect();
    let removed: Vec<NodeId> = old.iter().map(|(id, _)| id).filter(|id| !new.contains(*id)).collect();
    let changed: Vec<NodeId> =
        new.iter().filter(|(id, e)| old.get(*id).is_some_and(|o| o.node != e.node)).map(|(id, _)| id).collect();
    if !added.is_empty() {
        changes.push(format!("Added {}", names(new, added)));
    }
    if !removed.is_empty() {
        changes.push(format!("Removed {}", names(old, removed)));
    }
    if !changed.is_empty() {
        changes.push(format!("Settings of {}", names(new, changed)));
    }
    let switched = |disabled: bool| -> Vec<NodeId> {
        new.iter()
            .filter(|(id, e)| e.disabled == disabled && old.get(*id).is_some_and(|o| o.disabled != disabled))
            .map(|(id, _)| id)
            .collect()
    };
    let (disabled, enabled) = (switched(true), switched(false));
    if !disabled.is_empty() {
        changes.push(format!("Disabled {}", names(new, disabled)));
    }
    if !enabled.is_empty() {
        changes.push(format!("Enabled {}", names(new, enabled)));
    }
    let edges_switched = |disabled: bool| {
        new.edges()
            .iter()
            .filter(|e| old.edges().contains(e))
            .filter(|e| new.is_edge_disabled(e) == disabled && old.is_edge_disabled(e) != disabled)
            .count()
    };
    for (count, what) in [(edges_switched(true), "disabled"), (edges_switched(false), "enabled")] {
        if count > 0 {
            changes.push(format!("{count} connection{} {what}", if count == 1 { "" } else { "s" }));
        }
    }
    let connected = new.edges().iter().filter(|e| !old.edges().contains(e)).count();
    let disconnected = old.edges().iter().filter(|e| !new.edges().contains(e)).count();
    if connected > 0 {
        changes.push(format!("{connected} connected"));
    }
    if disconnected > 0 {
        changes.push(format!("{disconnected} disconnected"));
    }
    (!changes.is_empty()).then(|| (label, format!("Version {}: {}", after.version, changes.join("; "))))
}

/// Milliseconds since `start`.
fn ms_since(start: Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}

impl<N: NodeSet, A: AnnotationSet> eframe::App for App<N, A> {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.last_cpu = frame.info().cpu_usage;
        self.show(ui);
    }

    /// Behind everything, in the theme's colours (eframe's default is near black whatever the theme).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.colours().surface.to_normalized_gamma_f32()
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, THEME_KEY, &self.themes[self.theme].name);
        eframe::set_value(storage, BASE_DIRECTORY_KEY, &self.base_directories.save());
    }
}

/// "1:02:03.4", or "02:03.4" under an hour.
fn clock_text(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0) as u64;
    let (hours, minutes, seconds, tenths) = (tenths / 36000, tenths / 600 % 60, tenths / 10 % 60, tenths % 10);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{tenths}")
    } else {
        format!("{minutes:02}:{seconds:02}.{tenths}")
    }
}

/// A version of the document: a snapshot of the graph with all its configuration, and the annotations.
struct Version<N: NodeSet, A> {
    snapshot: Snapshot<N>,
    annotations: Arc<Annotations<A>>,
}

/// Undo and redo step between versions. A new version is taken whenever anything has changed (including
/// node settings) and the user isn't mid-gesture (dragging, or typing in a field), so a whole drag or edit
/// undoes in one step.
struct History<N: NodeSet, A> {
    current: Version<N, A>,
    undo: Vec<Version<N, A>>,
    redo: Vec<Version<N, A>>,
    versions: u64,
}

const HISTORY_LIMIT: usize = 200;

impl<N: NodeSet, A: AnnotationSet> History<N, A> {
    fn new(graph: &Graph<N>, annotations: &Annotations<A>) -> Self {
        let current =
            Version { snapshot: Snapshot::new(0, graph.clone(), None), annotations: Arc::new(annotations.clone()) };
        History { current, undo: Vec::new(), redo: Vec::new(), versions: 0 }
    }

    fn record(&mut self, graph: &Graph<N>, annotations: &Annotations<A>, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
            self.commit(graph, annotations);
        }
    }

    /// Takes a new version if anything has changed, even mid-gesture.
    fn commit(&mut self, graph: &Graph<N>, annotations: &Annotations<A>) {
        if *graph == *self.current.snapshot.graph && *annotations == *self.current.annotations {
            return;
        }
        self.versions += 1;
        let version = Version {
            snapshot: Snapshot::new(self.versions, graph.clone(), Some(&self.current.snapshot)),
            annotations: Arc::new(annotations.clone()),
        };
        self.undo.push(std::mem::replace(&mut self.current, version));
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn restored(&self) -> (Graph<N>, Annotations<A>) {
        ((*self.current.snapshot.graph).clone(), (*self.current.annotations).clone())
    }

    fn undo(&mut self) -> Option<(Graph<N>, Annotations<A>)> {
        let previous = self.undo.pop()?;
        let recording = self.current.snapshot.recording.clone();
        self.redo.push(std::mem::replace(&mut self.current, previous));
        self.set_recording(recording); // undoing an edit doesn't start or stop recording
        Some(self.restored())
    }

    /// Makes the current version recording or not, as a new version (not one to undo to). Returns the old version
    /// and the new, if it changed.
    fn set_recording(&mut self, recording: Option<Arc<engine::Recording>>) -> Option<(u64, u64)> {
        let old = self.current.snapshot.version;
        let same = match (&self.current.snapshot.recording, &recording) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        if same {
            return None;
        }
        self.versions += 1;
        self.current.snapshot = self.current.snapshot.with_recording(self.versions, recording);
        Some((old, self.versions))
    }

    fn redo(&mut self) -> Option<(Graph<N>, Annotations<A>)> {
        let next = self.redo.pop()?;
        let recording = self.current.snapshot.recording.clone();
        self.undo.push(std::mem::replace(&mut self.current, next));
        self.set_recording(recording);
        Some(self.restored())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::Role;
    use crate::core::layout::WindowState;
    use crate::standard_components::themes::THEMES;
    use crate::{AnnotationTypes, Nodes};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    /// The app on the example graph (which the tests don't save). Run with UPDATE_SNAPSHOTS=1 to accept changes; see
    /// tests/snapshots/.
    type TestApp = App<Nodes, AnnotationTypes>;

    mod tour;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rig.json")
    }

    fn harness() -> Harness<'static, TestApp> {
        let mut app = TestApp::open(Some(example()), THEMES)
            .with_quick_actions(crate::components::quick_actions::QUICK_ACTIONS)
            .with_base_directories(crate::components::BASE_DIRECTORIES);
        // Most tests work on the graph.
        app.layout.mode = Mode::Advanced;
        assert!(app.status.is_none(), "the example opens");
        let mut harness = Harness::builder()
            .with_size([1400.0, 850.0])
            .wgpu()
            .build_ui_state(|ui, app: &mut TestApp| app.show(ui), app);
        harness.run_steps(3);
        harness
    }

    /// Adds an annotation below the example's nodes, and frames them all.
    fn with_annotation(
        harness: &mut Harness<TestApp>,
        kind: AnnotationTypes,
        pos: [f32; 2],
        size: [f32; 2],
    ) -> crate::core::annotation::AnnotationId {
        let id = harness.state_mut().annotations.add(crate::core::annotation::Annotation { pos, size, kind });
        harness.state_mut().canvas.frame_all();
        harness.run_steps(3);
        id
    }

    fn node_named(app: &TestApp, label: &str) -> NodeId {
        app.graph.iter().find(|(_, e)| e.label == label).map(|(id, _)| id).expect("node")
    }

    /// Screen position of a point given relative to a node's top-left corner, in scene units.
    fn on_node(harness: &Harness<TestApp>, label: &str, offset: [f32; 2]) -> egui::Pos2 {
        let app = harness.state();
        let pos = app.graph.get(node_named(app, label)).unwrap().pos;
        app.canvas.to_screen(egui::pos2(pos[0] + offset[0], pos[1] + offset[1]))
    }

    fn pointer(harness: &mut Harness<TestApp>, pos: egui::Pos2, pressed: Option<bool>) {
        harness.event(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            let button = egui::PointerButton::Primary;
            harness.event(egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
        }
        harness.run_steps(2);
    }

    /// The first output row's port, on the right edge.
    const FIRST_OUTPUT: [f32; 2] = [NODE_WIDTH, HEADER + 5.0 + 11.0];

    #[test]
    fn example_graph() {
        let mut harness = harness();
        harness.snapshot("graph_example");
    }

    #[test]
    fn drawing_a_sticky_note() {
        let mut harness = harness();
        let notes = harness.state().annotations.ids().count();
        let ctx = harness.ctx.clone();
        harness.state_mut().perform(&ctx, Action::DrawTool(0));
        let start = on_node(&harness, "Camera", [0.0, 140.0]);
        pointer(&mut harness, start, Some(true));
        pointer(&mut harness, start + egui::vec2(100.0, 50.0), None);
        pointer(&mut harness, start + egui::vec2(220.0, 110.0), None);
        harness.snapshot("graph_drawing_note");
        pointer(&mut harness, start + egui::vec2(220.0, 110.0), Some(false));

        let app = harness.state();
        assert_eq!(app.annotations.ids().count(), notes + 1);
        assert_eq!(app.canvas.tool, Tool::Select, "back to selecting once drawn");
        let id = *app.selection.annotations.first().expect("the new note is selected");
        let size = app.annotations.get(id).unwrap().size;
        assert!(size[0] > 150.0 && size[1] > 70.0, "drawn to the dragged size, got {size:?}");
    }

    #[test]
    fn legend_collapses() {
        let mut harness = harness();
        assert!(harness.state().canvas.legend_open);
        harness.get_by_label("Legend").click();
        harness.run_steps(2);
        assert!(!harness.state().canvas.legend_open);
        harness.snapshot("graph_legend_collapsed");
    }

    #[test]
    fn legend_lists_only_edge_types_on_the_graph() {
        let names = |graph: &Graph<Nodes>| -> Vec<(&str, Vec<&str>)> {
            canvas::legend_entries::<Nodes>(&graph.resolve())
                .into_iter()
                .map(|(section, _, kinds)| (section, kinds.iter().map(|k| k.name).collect()))
                .collect()
        };
        let mut graph = Graph::<Nodes>::default();
        assert!(names(&graph).is_empty(), "nothing to explain on an empty graph");
        let camera = Nodes::TYPES.iter().position(|t| t.name == "Camera").unwrap();
        graph.add(Nodes::create(camera), [0.0, 0.0]);
        assert_eq!(names(&graph), vec![("Data", vec!["video", "camera stats"]), ("Settings", vec!["camera"])]);
        let detector = Nodes::TYPES.iter().position(|t| t.name == "Spike Detector").unwrap();
        graph.add(Nodes::create(detector), [0.0, 0.0]);
        assert_eq!(
            names(&graph),
            vec![
                ("Data", vec!["continuous", "spikes", "video", "camera stats"]),
                ("Settings", vec!["thresholds", "camera"])
            ]
        );
    }

    #[test]
    fn light_theme() {
        let mut harness = harness();
        // From the toolbar's theme button.
        harness.get_by_label("Theme").click();
        harness.run_steps(2);
        harness.snapshot("toolbar_theme_menu");
        harness.get_by_label("Light").click();
        harness.run_steps(3);
        assert_eq!(THEMES[harness.state().theme].name, "Light");
        harness.snapshot("graph_light");
    }

    #[test]
    fn china_theme() {
        let mut app = TestApp::open(Some(example()), crate::components::THEMES);
        app.layout.mode = Mode::Advanced;
        app.theme = 2;
        let (spikes, detector) = (node_named(&app, "Spike Viewer"), node_named(&app, "Spike Detector"));
        app.layout.open(spikes, Some(Pane::Home));
        app.layout.open(detector, Some(Pane::Home));
        app.selection.only(detector);
        let mut harness = Harness::builder()
            .with_size([1400.0, 850.0])
            .wgpu()
            .build_ui_state(|ui, app: &mut TestApp| app.show(ui), app);
        crate::components::china::CHINA.apply(&harness.ctx);
        harness.run_steps(3);
        harness.snapshot("graph_china");
    }

    #[test]
    fn zoomed_in_text_stays_sharp() {
        let mut harness = harness();
        let at = on_node(&harness, "Spike Detector", [100.0, 40.0]);
        pointer(&mut harness, at, None);
        harness.event(egui::Event::Zoom(2.5));
        harness.run_steps(2);
        harness.snapshot("graph_zoomed");
    }

    #[test]
    fn dragging_a_wire_highlights_compatible_ports() {
        let mut harness = harness();
        let start = on_node(&harness, "Acquisition Board", FIRST_OUTPUT);
        pointer(&mut harness, start, Some(true));
        pointer(&mut harness, start + egui::vec2(80.0, -80.0), None);
        pointer(&mut harness, start + egui::vec2(150.0, -170.0), None);
        harness.snapshot("graph_dragging_wire");

        // Dropped on empty canvas: offers what could go there, then adds and connects the choice.
        pointer(&mut harness, start + egui::vec2(150.0, -170.0), Some(false));
        assert!(harness.state().menu.is_some());
        harness.snapshot("graph_create_menu");
        let edges = harness.state().graph.edges().len();
        harness.get_by_label("Continuous Gain").click();
        harness.run_steps(2);
        let app = harness.state();
        assert_eq!(app.graph.edges().len(), edges + 1);
        // Named after the example's own.
        let gain = Port { node: node_named(app, "Continuous Gain 2"), role: Role::Input, index: 0 };
        let board = node_named(app, "Acquisition Board");
        assert_eq!(app.graph.edge_into(gain).map(|e| e.from.node), Some(board));
    }

    #[test]
    fn windows_dock_and_come_back() {
        let mut harness = harness();
        let lfp = node_named(harness.state(), "LFP Viewer");
        let spikes = node_named(harness.state(), "Spike Viewer");
        let detector = node_named(harness.state(), "Spike Detector");
        let app = harness.state_mut();
        // Opening docks, from the graph: the first window in a column on its right, the next below that.
        app.layout.toggle(spikes, Some(Pane::Home));
        app.layout.toggle(lfp, Some(Pane::Home));
        app.layout.toggle(detector, Some(Pane::Home));
        app.layout.float(detector);
        app.selection.only(detector);
        harness.run_steps(3);
        harness.snapshot("graph_windows");
        THEMES[1].apply(&harness.ctx);
        harness.state_mut().theme = 1;
        harness.run_steps(3);
        harness.snapshot("graph_windows_light");
        THEMES[0].apply(&harness.ctx);
        harness.state_mut().theme = 0;
        assert_eq!(harness.query_all_by_label("Undock").count(), 2, "each group of docked windows can undock");
        assert_eq!(harness.query_all_by_label("Dock").count(), 1, "the floating window can be docked");

        // Closing a docked window keeps its place for when it's reopened.
        let app = harness.state_mut();
        assert_eq!(app.layout.window(lfp), WindowState::Docked);
        assert_eq!(app.layout.window(detector), WindowState::Floating);
        app.layout.toggle(spikes, Some(Pane::Home));
        assert_eq!(app.layout.window(spikes), WindowState::Hidden);
        app.layout.toggle(spikes, Some(Pane::Home));
        assert_eq!(app.layout.window(spikes), WindowState::Docked);
        app.layout.dock(detector, Some(Pane::Home));
        assert_eq!(app.layout.window(detector), WindowState::Docked);
    }

    #[test]
    fn floating_window_moves_by_its_header() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        harness.state_mut().layout.toggle(detector, Some(Pane::Home));
        harness.state_mut().layout.float(detector);
        harness.run_steps(3);
        let area = |h: &Harness<TestApp>| h.ctx.memory(|m| m.area_rect(Id::unique(("node-window", detector))));
        let before = area(&harness).expect("window shown");
        // Grab the bar to the right of the tab, clear of the tab's close and dock buttons.
        let grab = egui::pos2(before.center().x + 30.0, before.top() + 12.0);
        pointer(&mut harness, grab, Some(true));
        pointer(&mut harness, grab + egui::vec2(-20.0, 5.0), None);
        pointer(&mut harness, grab + egui::vec2(-60.0, 20.0), None);
        pointer(&mut harness, grab + egui::vec2(-60.0, 20.0), Some(false));
        let moved = area(&harness).unwrap().min - before.min;
        assert!(moved.x < -50.0 && moved.y > 15.0, "window should follow the drag, moved {moved:?}");
    }

    #[test]
    fn play_runs_the_graph() {
        let mut harness = harness();
        harness.get_by_label("Play (Space)").click();
        harness.run_steps(2);
        assert!(harness.state().runner.is_some());
        let stats = engine::tests::wait_for_passes(harness.state().runner.as_ref().unwrap(), 1);
        assert!(stats.completed > 0, "{stats:?}");
        // Editing a setting carries the run on with the new version of the graph.
        let first = harness.state().runner.as_ref().unwrap().graph().clone();
        let gain_free = node_named(harness.state(), "Spike Detector");
        if let Some(entry) = harness.state_mut().graph.get_mut(gain_free) {
            entry.node.set_setting(0, crate::standard_components::spikes::Thresholds(vec![60.0]).into());
        }
        harness.run_steps(2);
        assert!(!Arc::ptr_eq(&first, &harness.state().runner.as_ref().unwrap().graph()));
        harness.key_press(Key::Space);
        harness.run_steps(2);
        assert!(harness.state().runner.is_none());
    }

    #[test]
    fn the_base_directory_is_in_the_toolbar_and_chosen_in_advanced_mode() {
        let mut harness = harness();
        // Subject, date and trial, in a directory that isn't there (so the next trial's the first).
        let saved = serde_json::json!({
            "chosen": "Subject, date and trial",
            "types": { "Subject, date and trial": { "directory": "/nowhere/recordings", "subject": "r2405" } },
        });
        let types = harness.state().base_directories.types();
        harness.state_mut().base_directories = BaseDirectories::new(types, Some(&saved));
        harness.run_steps(2);
        let place = harness.state().base_directories.place().unwrap();
        assert_eq!(
            place.path(),
            Path::new("/nowhere/recordings/r2405/2026-01-01/2026-01-01 a"),
            "the subject's, named the day's first"
        );
        harness.get_by_label("512 GB free");
        harness.get_by_label("Where recordings go").click();
        harness.run_steps(2);
        harness.snapshot("toolbar_base_directory");
        harness.get_by_label("A directory").click();
        harness.run_steps(2);
        assert_eq!(harness.state().base_directories.chosen(), Some(0));
    }

    #[test]
    fn recordings_keep_the_settings_they_were_made_with() {
        let start = chrono::Local::now();
        let named = Place { directory: "/r/r2405".into(), name: Some("2026-01-01 a".into()) };
        assert_eq!(settings_path(&named, &start), PathBuf::from("/r/r2405/2026-01-01 a.ephys-rs.json"));
        let unnamed = settings_path(&Place::directory("/r"), &start);
        assert!(unnamed.starts_with("/r") && unnamed.to_string_lossy().ends_with(".ephys-rs.json"));

        let harness = harness();
        let directory = std::env::temp_dir().join(format!("ephys-rs-settings-{}", std::process::id()));
        harness.state().save_with_recording(&Place { directory: directory.clone(), name: Some("trial a".into()) });
        crate::core::background::wait();
        let saved = std::fs::read_to_string(directory.join("trial a.ephys-rs.json")).unwrap();
        let file: SaveFile<Nodes, crate::AnnotationTypes> = serde_json::from_str(&saved).unwrap();
        assert_eq!(file.graph.iter().count(), harness.state().graph.iter().count(), "the graph, as it was");
        assert_eq!(file.base_directory, Some(harness.state().base_directories.save()), "and where it went");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn files_keep_the_base_directories() {
        let trials = "Subject, date and trial";
        let mut harness = harness();
        let saved = serde_json::json!({
            "chosen": trials,
            "types": {
                "A directory": { "directory": "/nowhere/else" },
                trials: { "directory": "/nowhere/recordings", "subject": "r2405" },
            },
        });
        let types = harness.state().base_directories.types();
        harness.state_mut().base_directories = BaseDirectories::new(types, Some(&saved));
        let path = std::env::temp_dir().join(format!("ephys-rs-file-{}.json", std::process::id()));
        let context = harness.ctx.clone();
        assert!(harness.state_mut().save_to(&context, path.clone()));
        assert!(!harness.state().unsaved());

        // Something else, in the app: what's chosen counts as a change to the file, and opening it brings it back.
        harness.state_mut().base_directories.choose(0);
        assert!(harness.state().unsaved(), "another one chosen");
        harness.state_mut().load(&path).unwrap();
        assert!(!harness.state().unsaved());
        assert_eq!(harness.state().base_directories.save(), saved, "which is in use, and what each has");

        // A file from before files kept them leaves them as they are.
        let mut old: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("base_directory").unwrap();
        std::fs::write(&path, old.to_string()).unwrap();
        harness.state_mut().base_directories.choose(0);
        harness.state_mut().load(&path).unwrap();
        assert_eq!(harness.state().base_directories.chosen(), Some(0));
        assert!(!harness.state().unsaved());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn dev_tools_show_the_plan() {
        let mut harness = harness();
        // However many cores the machine has.
        harness.state_mut().workers = 4;
        // However fast it runs.
        harness.state_mut().frame_times.pause();
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::I);
        // Other tests log at the same time, so the console's filtered to nothing, to look the same every time.
        harness.state_mut().dev_tools.hide_console_lines();
        harness.run_steps(3);
        assert!(harness.state().layout.is_shown(Pane::DevTools));
        // The plan's name is where it is in memory, which changes from run to run.
        let options = egui_kittest::SnapshotOptions::new().max_failed_pixels(400);
        harness.snapshot_options("dev_tools", &options);
        // Each section collapses.
        assert!(harness.query_by_label("Clear").is_some());
        harness.get_by_role_and_label(egui::accesskit::Role::DisclosureTriangle, "Console").click();
        harness.run_steps(2);
        assert!(harness.query_by_label("Clear").is_none(), "the console's collapsed");
    }

    #[test]
    fn settings_are_locked_while_recording() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        let threshold = |app: &TestApp| match app.graph.get(detector).map(|e| &e.node) {
            Some(Nodes::SpikeDetector(d)) => d.thresholds.get(0),
            _ => unreachable!(),
        };
        let before = threshold(harness.state());
        harness.state_mut().set_recording(Some(Arc::new(engine::Recording::default())));
        assert!(!harness.state().enabled(Action::Undo) && !harness.state().enabled(Action::AddNode));
        if let Some(Nodes::SpikeDetector(d)) = harness.state_mut().graph.get_mut(detector).map(|e| &mut e.node) {
            d.thresholds.set(0, before + 1.0);
        }
        harness.state_mut().graph.get_mut(detector).unwrap().pos[0] += 50.0;
        harness.run_steps(2);
        let app = harness.state();
        assert_eq!(threshold(app), before, "the setting's put back");
        assert!(app.graph.get(detector).unwrap().pos[0] > 0.0, "but nodes can still move");
        harness.state_mut().set_recording(None);
        assert!(harness.state().enabled(Action::AddNode));
    }

    #[test]
    fn a_box_is_grabbed_by_its_title_not_its_inside() {
        let mut harness = harness();
        let kind = AnnotationTypes::Box(Default::default());
        let id = with_annotation(&mut harness, kind, [0.0, 900.0], [600.0, 300.0]);
        let pos = |h: &Harness<TestApp>| h.state().annotations.get(id).unwrap().pos;
        let drag = |h: &mut Harness<TestApp>, scene: [f32; 2]| {
            let start = h.state().canvas.to_screen(egui::pos2(scene[0], scene[1]));
            pointer(h, start, Some(true));
            pointer(h, start + egui::vec2(20.0, 10.0), None);
            pointer(h, start + egui::vec2(40.0, 20.0), None);
            pointer(h, start + egui::vec2(40.0, 20.0), Some(false));
        };
        let before = pos(&harness);
        // Inside, away from the nodes in it: a marquee, not a move.
        drag(&mut harness, [before[0] + 120.0, before[1] + 280.0]);
        assert_eq!(pos(&harness), before);
        // By its title.
        drag(&mut harness, [before[0] + 300.0, before[1] + 10.0]);
        assert!(pos(&harness)[0] > before[0], "moved by its title");
    }

    #[test]
    fn text_pasted_on_the_canvas_becomes_an_annotation() {
        let mut harness = harness();
        let before = harness.state().annotations.ids().count();
        harness.event(egui::Event::Paste("  # Protocol\n- check impedances  ".into()));
        harness.run_steps(2);
        let app = harness.state();
        assert_eq!(app.annotations.ids().count(), before + 1);
        let &id = app.selection.annotations.iter().next().expect("selected");
        let AnnotationTypes::Markdown(note) = &app.annotations.get(id).unwrap().kind else { panic!("markdown") };
        assert_eq!(note.text, "# Protocol\n- check impedances", "trimmed");
    }

    #[test]
    fn settings_pasted_the_usual_way_are_pasted_as_settings() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        let threshold = |app: &TestApp| match app.graph.get(detector).map(|e| &e.node) {
            Some(Nodes::SpikeDetector(d)) => d.thresholds.get(0),
            _ => unreachable!(),
        };
        // Its settings copied, with another threshold.
        let copied = {
            let app = harness.state_mut();
            let before = app.graph.get(detector).unwrap().node.clone();
            if let Some(entry) = app.graph.get_mut(detector) {
                entry.node.set_setting(0, crate::standard_components::spikes::Thresholds(vec![77.0]).into());
            }
            let copied = paste_settings::copy(&app.graph, &[detector]);
            app.graph.get_mut(detector).unwrap().node = before;
            copied
        };
        let annotations = harness.state().annotations.ids().count();
        harness.state_mut().selection.only(detector);
        harness.event(egui::Event::Paste(copied));
        harness.run_steps(2);
        assert_eq!(threshold(harness.state()), 77.0, "pasted straight onto the node selected");
        assert_eq!(harness.state().annotations.ids().count(), annotations, "not as a note");
        // Other JSON is only text.
        harness.event(egui::Event::Paste("{\"format\": \"something else\"}".into()));
        harness.run_steps(2);
        assert_eq!(harness.state().annotations.ids().count(), annotations + 1);
    }

    #[test]
    fn an_arrows_ends_are_dragged_one_at_a_time() {
        let mut harness = harness();
        let id =
            with_annotation(&mut harness, AnnotationTypes::Arrow(Default::default()), [0.0, 900.0], [300.0, 150.0]);
        let points = |h: &Harness<TestApp>| {
            let annotation = h.state().annotations.get(id).unwrap();
            let AnnotationTypes::Arrow(arrow) = &annotation.kind else { unreachable!() };
            let (start, end) = arrow.ends(annotation.rect());
            [start, end]
        };
        let [start, end] = points(&harness);
        let at = harness.state().canvas.to_screen(end);
        pointer(&mut harness, at, Some(true));
        pointer(&mut harness, at + egui::vec2(20.0, 20.0), None);
        pointer(&mut harness, at + egui::vec2(-40.0, 60.0), None);
        pointer(&mut harness, at + egui::vec2(-40.0, 60.0), Some(false));
        let [new_start, new_end] = points(&harness);
        assert!(new_start.distance(start) < 0.5, "the start stays put");
        assert!(new_end.y > end.y + 20.0 && new_end.x < end.x, "the end follows the drag");
    }

    #[test]
    fn annotations_but_boxes_go_above_the_graph() {
        use crate::core::annotation::{Annotation, AnnotationType as _};
        use crate::standard_components::annotations::image::Image;
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        let over = harness.state().graph.get(detector).unwrap().pos;
        let pixels = egui::ColorImage::new([40, 30], vec![egui::Color32::RED; 1200]);
        let (_, image, _) = Image::pasted(&crate::core::annotation::Pasted::Image(&pixels)).unwrap();
        let annotation = Annotation { pos: over, size: [80.0, 60.0], kind: AnnotationTypes::Image(image) };
        let id = harness.state_mut().annotations.add(annotation);
        harness.run_steps(2);
        let at = on_node(&harness, "Spike Detector", [20.0, 20.0]);
        pointer(&mut harness, at, Some(true));
        pointer(&mut harness, at, Some(false));
        let selection = &harness.state().selection;
        assert!(selection.annotations.contains(&id), "the image, over the node, is what's clicked");
        assert!(selection.nodes.is_empty());
    }

    #[test]
    fn a_click_selects_a_node_and_another_opens_its_window() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        let at = on_node(&harness, "Spike Detector", [60.0, 40.0]);
        pointer(&mut harness, at, Some(true));
        pointer(&mut harness, at, Some(false));
        assert!(harness.state().selection.nodes.contains(&detector));
        assert_eq!(harness.state().layout.window(detector), WindowState::Closed, "a click only selects it");
        // Clicked again, once selected.
        pointer(&mut harness, at, Some(true));
        pointer(&mut harness, at, Some(false));
        harness.run_steps(2);
        assert_eq!(harness.state().layout.window(detector), WindowState::Docked);
        // Its default window: its settings, and its thresholds' value.
        assert!(harness.query_all_by_label_contains("µV").next().is_some());
    }

    #[test]
    fn the_annotation_pane_is_there_while_an_annotation_is_selected() {
        let mut harness = harness();
        assert!(!harness.state().layout.is_shown(Pane::Annotation));
        let kind = AnnotationTypes::Markdown(Default::default());
        let note = with_annotation(&mut harness, kind, [0.0, 900.0], [260.0, 160.0]);
        harness.state_mut().selection.only_annotation(note);
        harness.run_steps(3);
        assert!(harness.state().layout.is_shown(Pane::Annotation));
        assert!(harness.query_all_by_label("Markdown").next().is_some(), "the note's settings");
        harness.snapshot("annotation_pane");
        assert!(harness.query_by_label("Add an image").is_some());
        harness.get_by_label("Markdown cheat sheet").click();
        harness.run_steps(2);
        harness.run_steps(2);
        // Open, the text makes room for it, so it's all in view.
        let last = harness.get_by_label("a line across").rect();
        assert!(last.bottom() < 850.0, "the cheat sheet shows: {last:?}");
        harness.snapshot("annotation_pane_cheat_sheet");
        harness.state_mut().selection.clear();
        harness.run_steps(3);
        assert!(!harness.state().layout.is_shown(Pane::Annotation));
    }

    #[test]
    fn says_where_the_last_crash_report_is() {
        let mut app = TestApp::open(None, THEMES);
        app.last_crash = Some(PathBuf::from("/reports/crash-2026-11-02-210040.txt"));
        let mut harness = Harness::builder()
            .with_size([1000.0, 600.0])
            .wgpu()
            .build_ui_state(|ui, app: &mut TestApp| app.show(ui), app);
        harness.run_steps(2);
        assert!(harness.query_by_label_contains("crash-2026-11-02-210040.txt").is_some(), "where it is");
        harness.snapshot("last_crash");
        harness.get_by_label("OK").click();
        harness.run_steps(2);
        assert!(harness.state().last_crash.is_none(), "said once");
    }

    #[test]
    fn quick_actions_build_the_graph_from_the_basic_view() {
        let mut app = TestApp::open(None, THEMES).with_quick_actions(crate::components::quick_actions::QUICK_ACTIONS);
        // Trials, when they're chosen, in a directory that isn't there (so they start from the first).
        let saved =
            serde_json::json!({ "types": { "Subject, date and trial": { "directory": "/nowhere/recordings" } } });
        let types = crate::components::BASE_DIRECTORIES;
        app.base_directories = BaseDirectories::new(types, Some(&saved));
        let mut harness = Harness::builder()
            .with_size([1400.0, 850.0])
            .wgpu()
            .build_ui_state(|ui, app: &mut TestApp| app.show(ui), app);
        harness.run_steps(3);
        assert_eq!(harness.state().layout.mode, Mode::Basic, "it starts in the basic view");
        harness.snapshot("basic_empty");
        let button = egui::accesskit::Role::Button;
        let offered = |h: &Harness<TestApp>, action: &str| h.query_by_role_and_label(button, action).is_some();
        // A pane open before goes behind the spike viewer. (The dev tools open as a tab beside home: home's brought
        // back to the front.)
        harness.state_mut().layout.toggle_dev_tools();
        harness.state_mut().layout.show(Pane::Home);
        harness.run_steps(2);
        assert_eq!(harness.state().base_directories.chosen(), Some(0));
        harness.get_by_role_and_label(button, "Standard LTX graph v0.5").click();
        harness.run_steps(3);
        let chosen = harness.state().base_directories.chosen().map(|i| types[i].name);
        assert_eq!(chosen, Some("Subject, date and trial"), "recording subjects' trials");
        for viewer in ["Spike Viewer", "LFP Viewer", "TTL Viewer", "Tracker View"] {
            let id = node_named(harness.state(), viewer);
            assert_eq!(harness.state().layout.window(id), WindowState::Docked, "the {viewer} opens");
        }
        let group = |h: &Harness<TestApp>, viewer: &str| {
            let id = node_named(h.state(), viewer);
            h.state().layout.tab_group(Pane::Node(id)).unwrap()
        };
        let layout = &harness.state().layout;
        let (tracker, spikes, eeg) =
            (group(&harness, "Tracker View"), group(&harness, "Spike Viewer"), group(&harness, "LFP Viewer"));
        assert!(tracker.1 && spikes.1 && eeg.1, "the viewers in front");
        assert_eq!(layout.tab_group(Pane::Home), Some((tracker.0, false)), "home behind the tracker");
        assert_eq!(layout.tab_group(Pane::DevTools), Some((spikes.0, false)), "the dev tools behind the spikes");
        assert_eq!(group(&harness, "TTL Viewer"), (eeg.0, false), "the TTLs behind the EEG");
        harness.snapshot("standard_ltx_layout");
        // The panes' toolbars, in the basic view, leave out what changes the graph.
        let buttons = |h: &Harness<TestApp>, label: &str| h.query_all_by_label(label).count();
        assert!(buttons(&harness, "Copy settings") > 0 && buttons(&harness, "Paste settings") > 0);
        for left_out in ["Enabled", "Show in basic view", "Delete"] {
            assert_eq!(buttons(&harness, left_out), 0, "no {left_out} in the basic view");
        }
        harness.state_mut().layout.toggle_dev_tools();
        harness.state_mut().layout.show(Pane::Home);
        harness.run_steps(2);
        assert!(harness.state().layout.is_shown(Pane::Home), "the basic view stays");
        assert!(!offered(&harness, "Standard LTX graph v0.5"), "not offered once done");
        assert!(offered(&harness, "Disable position tracking") && offered(&harness, "Use synthetic data"));
        harness.snapshot("basic_after_quick_actions");
        harness.get_by_role_and_label(button, "Disable position tracking").click();
        harness.run_steps(2);
        assert!(offered(&harness, "Enable position tracking") && !offered(&harness, "Disable position tracking"));
        let tracker = node_named(harness.state(), "Tracker View");
        assert_eq!(harness.state().layout.window(tracker), WindowState::Hidden, "the tracker's window closes");
        harness.snapshot("basic_positions_disabled");
        // Undone in one step.
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        harness.run_steps(2);
        assert!(offered(&harness, "Disable position tracking"));
        // Advanced mode, with the graph, and back.
        harness.get_by_label_contains("Advanced mode").click();
        harness.run_steps(2);
        assert_eq!(harness.state().layout.mode, Mode::Advanced);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
        harness.run_steps(2);
        assert_eq!(harness.state().layout.mode, Mode::Basic);

        // The basic view lists viewers, not other nodes, unless set otherwise.
        let card = |h: &Harness<TestApp>, label: &str| {
            h.query_by_role_and_label(egui::accesskit::Role::Button, label).is_some()
        };
        assert!(card(&harness, "Spike Viewer") && !card(&harness, "Spike Detector"));
        let detector = node_named(harness.state(), "Spike Detector");
        let viewer = node_named(harness.state(), "Spike Viewer");
        harness.state_mut().graph.get_mut(detector).unwrap().basic = Some(true);
        harness.state_mut().graph.get_mut(viewer).unwrap().basic = Some(false);
        harness.run_steps(2);
        assert!(!card(&harness, "Spike Viewer") && card(&harness, "Spike Detector"));

        // Closed, Home brings it back: in advanced mode, the graph.
        let ctx = harness.ctx.clone();
        harness.state_mut().perform(&ctx, Action::Advanced);
        harness.state_mut().perform(&ctx, Action::CloseHome);
        harness.run_steps(2);
        assert!(!harness.state().layout.is_shown(Pane::Home));
        harness.get_by_label_contains("Home: the graph").click();
        harness.run_steps(2);
        assert!(harness.state().layout.is_shown(Pane::Home) && harness.state().layout.mode == Mode::Advanced);
    }

    #[test]
    fn changes_to_the_run_are_described() {
        let mut harness = harness();
        // Without the writers, so recording doesn't write files.
        for writer in ["Write LTX", "Write Raw"] {
            let id = node_named(harness.state(), writer);
            harness.state_mut().graph.remove(id);
        }
        let ctx = harness.ctx.clone();
        harness.state_mut().perform(&ctx, Action::Play);
        harness.run_steps(2);
        let detector = node_named(harness.state(), "Spike Detector");
        if let Some(Nodes::SpikeDetector(d)) = harness.state_mut().graph.get_mut(detector).map(|e| &mut e.node) {
            d.thresholds.set(0, 70.0);
        }
        harness.run_steps(3);
        let viewer = node_named(harness.state(), "TTL Viewer");
        harness.state_mut().graph.get_mut(viewer).unwrap().disabled = true;
        harness.run_steps(3);
        harness.state_mut().set_recording(Some(Arc::new(engine::Recording::default())));
        harness.run_steps(3);
        let marks: Vec<String> = harness.state().frame_times.marks.iter().map(|m| m.detail.clone()).collect();
        assert!(marks.iter().any(|m| m.contains("Settings of Spike Detector")), "{marks:?}");
        assert!(marks.iter().any(|m| m.contains("Recording started")), "{marks:?}");
        assert!(marks.iter().any(|m| m.contains("Disabled TTL Viewer")), "{marks:?}");
        // And marked again once a pass has used them.
        let used = |h: &Harness<TestApp>| {
            let marks = &h.state().frame_times.marks;
            marks.iter().any(|m| m.first_use && m.detail.contains("Recording started"))
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !used(&harness) && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.run_steps(1);
        }
        assert!(used(&harness), "the first pass recording");
        harness.state_mut().set_recording(None);
        harness.state_mut().perform(&ctx, Action::Play);
    }

    #[test]
    fn nodes_are_disabled_from_their_menu() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        let at = on_node(&harness, "Spike Detector", [60.0, 40.0]);
        harness.event(egui::Event::PointerMoved(at));
        let button = egui::PointerButton::Secondary;
        let modifiers = Modifiers::NONE;
        harness.event(egui::Event::PointerButton { pos: at, button, pressed: true, modifiers });
        harness.event(egui::Event::PointerButton { pos: at, button, pressed: false, modifiers });
        harness.run_steps(2);
        harness.get_by_label("Disable").click();
        harness.run_steps(2);
        assert!(harness.state().graph.get(detector).unwrap().disabled);
        let viewer = node_named(harness.state(), "Spike Viewer");
        assert!(harness.state().graph.resolve()[&viewer].input_streams[0].is_none(), "nothing comes from it");
        harness.snapshot("graph_disabled_node");

        // A selected edge, from the Edit menu's Disable or Enable.
        let edge = harness.state().graph.edges()[0];
        harness.state_mut().selection.clear();
        harness.state_mut().selection.edge = Some(edge);
        let ctx = harness.ctx.clone();
        harness.state_mut().perform(&ctx, Action::ToggleDisabled);
        assert!(harness.state().graph.is_edge_disabled(&edge));
        harness.state_mut().perform(&ctx, Action::ToggleDisabled);
        assert!(!harness.state().graph.is_edge_disabled(&edge));
        // And from its window's toggle.
        harness.state_mut().graph.get_mut(detector).unwrap().disabled = false;
        harness.state_mut().layout.open(detector, Some(Pane::Home));
        harness.run_steps(2);
        harness.get_by_label("Enabled").click();
        harness.run_steps(2);
        assert!(harness.state().graph.get(detector).unwrap().disabled);
    }

    #[test]
    fn settings_are_copied_and_pasted_after_review() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        let copied = paste_settings::copy(&harness.state().graph, &[detector]);
        let threshold = |h: &Harness<TestApp>| match h.state().graph.get(detector).map(|e| &e.node) {
            Some(Nodes::SpikeDetector(d)) => d.thresholds.get(0),
            _ => unreachable!(),
        };
        let before = threshold(&harness);
        if let Some(Nodes::SpikeDetector(d)) = harness.state_mut().graph.get_mut(detector).map(|e| &mut e.node) {
            d.thresholds.set(0, before + 20.0);
        }
        let paste = paste_settings::PasteSettings::new(&copied, &harness.state().graph).unwrap();
        harness.state_mut().paste = Some(paste);
        harness.run_steps(2);
        assert!(harness.query_by_label("thresholds").is_some(), "says what changes");
        harness.snapshot("paste_settings");
        harness.get_by_label("Paste to 1 node").click();
        harness.run_steps(2);
        assert_eq!(threshold(&harness), before, "pasted back");
        assert!(harness.state().paste.is_none());

        // Onto a node (from its window, say), one node's settings go straight on.
        if let Some(Nodes::SpikeDetector(d)) = harness.state_mut().graph.get_mut(detector).map(|e| &mut e.node) {
            d.thresholds.set(0, before + 20.0);
        }
        let ctx = harness.ctx.clone();
        harness.state_mut().paste_text(&ctx, &copied, Some(detector));
        assert_eq!(threshold(&harness), before, "pasted straight on");
        assert!(harness.state().paste.is_none(), "without the window");
        // Not onto a node of another type.
        let viewer = node_named(harness.state(), "Spike Viewer");
        let graph_before = harness.state().graph.clone();
        harness.state_mut().paste_text(&ctx, &copied, Some(viewer));
        assert!(harness.state().graph == graph_before && harness.state().paste.is_none());
        // Several nodes' settings, it asks.
        let both = paste_settings::copy(&harness.state().graph, &[detector, viewer]);
        harness.state_mut().paste_text(&ctx, &both, Some(detector));
        assert!(harness.state().paste.is_some());
    }

    #[test]
    fn panes_have_settings_in_advanced_mode() {
        let mut harness = harness();
        // In basic mode: no settings, enabling or deleting in panes.
        harness.state_mut().layout.mode = Mode::Basic;
        let viewer = node_named(harness.state(), "Spike Viewer");
        harness.state_mut().layout.open(viewer, Some(Pane::Home));
        harness.run_steps(2);
        assert!(harness.query_by_label("Settings").is_none() && harness.query_by_label("Enabled").is_none());
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
        harness.run_steps(2);
        harness.get_by_label("Settings").click();
        harness.run_steps(2);
        harness.get_by_label("Show in basic mode").click();
        harness.run_steps(2);
        assert_eq!(harness.state().graph.get(viewer).unwrap().basic, Some(false), "taken out of basic mode");
    }

    #[test]
    fn viewers_trigger_auto_detect_and_proposals_are_applied() {
        let mut harness = harness();
        for writer in ["Write LTX", "Write Raw"] {
            let id = node_named(harness.state(), writer);
            harness.state_mut().graph.remove(id);
        }
        let (viewer, gain) = (node_named(harness.state(), "Spike Viewer"), node_named(harness.state(), "Spike Gain"));
        harness.state_mut().layout.open(viewer, Some(Pane::Home));
        harness.run_steps(2);
        let ctx = harness.ctx.clone();
        harness.state_mut().perform(&ctx, Action::Play);
        harness.run_steps(2);
        // Just the gains, from the viewer's toolbar.
        if let Some(Nodes::SpikeViewer(v)) = harness.state_mut().graph.get_mut(viewer).map(|e| &mut e.node) {
            v.detect = crate::standard_components::spikes::spike_viewer::Detect::Gains;
        }
        harness.run_steps(2);
        // Not the Run menu's: the viewer's, drawn after it.
        harness.get_all_by_label("Run").last().expect("the viewer's Run").click();
        harness.run_steps(2);
        use crate::standard_components::auto_detect::AUTO_DETECT;
        let progress =
            |h: &Harness<TestApp>| h.state().runner.as_ref().unwrap().exchange().trigger_progress(gain, AUTO_DETECT);
        assert!(progress(&harness).is_some(), "the Spike Gain's auto detect, fired");
        // While it's going, it can be stopped.
        harness.get_by_label("Stop").click();
        harness.run_steps(2);
        assert_eq!(progress(&harness), None, "stopped");
        // What the passes propose is applied, as an edit.
        let gains = crate::standard_components::spikes::Gains { gains: vec![0.25], ..Default::default() };
        harness.state().runner.as_ref().unwrap().exchange().propose(
            gain,
            0,
            Box::new(crate::Setting::Gains(gains.clone())),
        );
        harness.run_steps(2);
        let settings = harness.state().graph.get(gain).unwrap().node.settings();
        assert_eq!(settings[0].value, crate::Setting::Gains(gains), "applied");
        harness.state_mut().perform(&ctx, Action::Play);
    }

    #[test]
    fn undo_restores_a_deleted_node() {
        let mut harness = harness();
        let id = node_named(harness.state(), "Spike Detector");
        let before = harness.state().snapshot().version;
        let edges = harness.state().graph.edges().len();
        harness.state_mut().selection.only(id);
        harness.key_press(Key::Backspace);
        harness.run_steps(2);
        assert!(!harness.state().graph.contains(id));
        assert!(harness.state().snapshot().version > before);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        harness.run_steps(2);
        assert!(harness.state().graph.contains(id));
        assert_eq!(harness.state().graph.edges().len(), edges);
    }
}
