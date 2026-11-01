//! The main window: menus and a toolbar, then a dockable layout of panes (the graph canvas, the inspector, and node
//! windows), then a status bar. Node windows open floating and can be docked.

mod actions;
mod canvas;
mod create_menu;
mod docking;
mod icons;
mod inspector;
#[cfg(target_os = "macos")]
mod mac_menu;
mod style;
mod toolbar;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Id, Key, Modifiers, Pos2, RichText, ViewportCommand};
use serde::{Deserialize, Serialize};

use crate::core::Components;
use crate::core::annotation::{Annotation, AnnotationId, AnnotationSet, Annotations};
use crate::core::engine::{self, Graph, NodeId, NodeSet, Port, PortRef, Resolved, Runner, Snapshot, Stats};
use crate::core::theme::{Colours, Theme};
use actions::Action;
use canvas::{Canvas, HEADER, NODE_WIDTH, Selection, Tool};
use create_menu::{Choice, CreateMenu};
use docking::{Layout, Pane, Panes};

/// Runs the app built from `C`'s components, opening the file at `path` (or an empty graph).
pub fn run<C: Components>(path: Option<PathBuf>) -> eframe::Result {
    let app = App::<C::Nodes, C::Annotations>::open(path, C::THEMES);
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png")).expect("valid icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ephys-rs")
            .with_inner_size([1400.0, 850.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "ephys-rs",
        options,
        Box::new(|cc| {
            let mut app = app;
            let saved: Option<String> = cc.storage.and_then(|s| eframe::get_value(s, THEME_KEY));
            if let Some(index) = saved.and_then(|name| app.themes.iter().position(|t| t.name == name)) {
                app.theme = index;
            }
            app.themes[app.theme].apply(&cc.egui_ctx);
            #[cfg(target_os = "macos")]
            {
                let checkable = |a| app.checked(a).is_some();
                app.mac_menu = mac_menu::MacMenu::install(&cc.egui_ctx, &checkable, C::Annotations::TYPES, C::THEMES);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Where the theme (by name) is kept among the app's preferences (which eframe saves, separately from graph files).
const THEME_KEY: &str = "theme";

/// Something a pane asked for, handled once the panes are drawn.
pub enum Event {
    /// Open the menu for adding a node, optionally connected to a port.
    OpenMenu {
        screen: Pos2,
        scene: Pos2,
        from: Option<Port>,
    },
    ToggleWindow(NodeId),
    FloatWindow(NodeId),
    Duplicate(NodeId),
    Disconnect(NodeId),
    Delete(NodeId),
    DeleteAnnotation(AnnotationId),
    DeleteSelection,
    Status(String),
}

/// The file: the graph, its annotations, and the window layout.
#[derive(Deserialize)]
#[serde(bound = "N: NodeSet, A: AnnotationSet")]
struct SaveFile<N, A> {
    graph: Graph<N>,
    #[serde(default)]
    annotations: Annotations<A>,
    /// Kept loose, so a layout from an older version is dropped rather than failing the whole file.
    #[serde(default)]
    layout: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(bound = "N: NodeSet, A: AnnotationSet")]
struct SaveFileRef<'a, N, A> {
    graph: &'a Graph<N>,
    annotations: &'a Annotations<A>,
    layout: &'a Layout,
}

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
    /// The window title as last set.
    title: String,
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
    /// Shown in the status bar until the given time (egui seconds).
    status: Option<Status>,
    history: History<N, A>,
    /// The graph running, while it is.
    runner: Option<Runner<N>>,
}

impl<N: NodeSet, A: AnnotationSet> App<N, A> {
    /// Opens the file at `path`, or an empty graph if there's no file there. `themes` mustn't be empty.
    pub fn open(path: Option<PathBuf>, themes: &'static [Theme]) -> Self {
        let annotations = Annotations::default();
        let graph = Graph::default();
        let mut app = App {
            path: None,
            saved: None,
            title: String::new(),
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
            status: None,
            runner: None,
        };
        if let Some(path) = path {
            if path.exists() {
                if let Err(e) = app.load(&path) {
                    app.status = Some(Status {
                        message: format!("Couldn't open {}: {e}", path.display()),
                        until: f64::INFINITY,
                        ok: false,
                    });
                }
            } else {
                // A new file, once saved.
                app.path = Some(path);
            }
        }
        app
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
        self.saved != Some(self.history.current.snapshot.version)
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
        self.layout = file.layout.and_then(|l| serde_json::from_value(l).ok()).unwrap_or_default();
        self.history = History::new(&file.graph, &file.annotations);
        self.graph = file.graph;
        self.annotations = file.annotations;
        self.selection.clear();
        self.menu = None;
        self.canvas.frame_all();
        self.path = Some(path.to_owned());
        self.saved = Some(self.history.current.snapshot.version);
        Ok(())
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

    fn save_to(&mut self, ctx: &egui::Context, path: PathBuf) -> bool {
        // Make sure the latest edits are in the version being marked as saved.
        self.history.commit(&self.graph, &self.annotations);
        let file = SaveFileRef { graph: &self.graph, annotations: &self.annotations, layout: &self.layout };
        let result = serde_json::to_string_pretty(&file)
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.set_done(ctx, format!("Saved {}", path.display()));
                self.path = Some(path);
                self.saved = Some(self.history.current.snapshot.version);
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
        self.saved = Some(self.history.current.snapshot.version);
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

    fn perform(&mut self, ctx: &egui::Context, action: Action) {
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
            Action::FrameAll => self.canvas.frame_all(),
            Action::ActualSize => self.canvas.reset_zoom(),
            Action::ResetLayout => self.layout = Layout::default(),
            // Goes through `confirm_close`, like closing the window.
            Action::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            Action::AddNode => {
                let at = self.canvas.view_centre();
                let screen = self.add_anchor.take().unwrap_or_else(|| self.canvas.to_screen(at));
                self.menu = Some(CreateMenu::new(screen, at, None));
            }
            Action::ToggleInspector => self.layout.toggle_pane(Pane::Inspector),
            Action::Play => {
                if self.runner.take().is_none() {
                    // Run whatever's latest, including edits not yet taken as a version.
                    self.history.commit(&self.graph, &self.annotations);
                    self.runner = Some(Runner::start(&self.history.current.snapshot, engine::default_workers()));
                }
            }
            Action::Theme(index) => {
                self.theme = index;
                self.themes[index].apply(ctx);
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
        let key = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if key(Key::Enter) {
            if let Some(id) = self.selection.single() {
                self.layout.toggle(id);
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
        let Some(runner) = &self.runner else { return };
        runner.update(&self.history.current.snapshot);
        // Keep the stats in the status bar fresh.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        ui.horizontal(|ui| {
            if let Some(runner) = &self.runner {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.run_stats(ui, &runner.stats());
                });
            }
            match (&self.status, self.canvas.hint()) {
                (Some(status), _) if now < status.until => {
                    let colours = self.colours();
                    let colour = if status.ok { colours.success } else { colours.warning };
                    ui.label(RichText::new(&status.message).color(colour))
                }
                (_, Some(hint)) => ui.label(hint),
                _ => ui
                    .weak("Double-click, right-click or Tab on the canvas to add a node · drag from a port to connect"),
            };
        });
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
        let colours = self.colours();
        for id in self.layout.floating() {
            let Some(entry) = self.graph.get(id) else { continue };
            let has_view = entry.node.has_view();
            let beside = self.canvas.to_screen(egui::pos2(entry.pos[0] + NODE_WIDTH + 20.0, entry.pos[1]));
            let title = entry.label.clone();
            let mut clicks = docking::HeaderClicks::default();
            let response = egui::Window::new(&title)
                .id(Id::unique(("node-window", id)))
                .title_bar(false)
                .collapsible(false)
                .frame(egui::Frame::window(&ctx.global_style()).inner_margin(0).corner_radius(2))
                .default_pos(beside)
                .default_size(if has_view { [820.0, 520.0] } else { [300.0, 200.0] })
                .resizable(has_view)
                .show(ctx, |ui| {
                    clicks = docking::floating_header(ui, &title);
                    egui::Frame::new().inner_margin(8).show(ui, |ui| {
                        docking::node_ui(ui, &mut self.graph, resolved, colours, self.runner.as_ref(), id);
                    });
                });
            if let Some(response) = response
                && (response.response.clicked() || response.response.drag_started())
            {
                self.selection.only(id);
            }
            if clicks.close {
                events.push(Event::ToggleWindow(id));
            }
            if clicks.dock {
                self.layout.dock(id);
            }
        }
    }

    fn handle(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::OpenMenu { screen, scene, from } => self.menu = Some(CreateMenu::new(screen, scene, from)),
            Event::ToggleWindow(id) => self.layout.toggle(id),
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
            Event::DeleteSelection => self.delete_selection(),
            Event::Status(message) => self.set_status(ctx, message),
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

    fn window_title(&mut self, ctx: &egui::Context) {
        let title = format!("{}{} — ephys-rs", self.file_name(), if self.unsaved() { " •" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
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
        let ctx = ui.ctx().clone();
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
        let resolved = self.graph.resolve();
        let open_windows = self.layout.open_windows();
        self.canvas.colours = self.colours();

        self.window_title(&ctx);
        self.confirm_close(&ctx);
        let mut chosen = None;
        if !self.native_menu() {
            egui::Panel::top("menu_bar").show(ui, |ui| chosen = self.menu_bar(ui));
        }
        egui::Panel::top("toolbar").show(ui, |ui| chosen = chosen.or(self.toolbar(ui)));
        if let Some(action) = chosen {
            self.perform(&ctx, action);
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let mut events = Vec::new();
        // Panes don't paint their own backgrounds, so this shows behind them.
        let colours = self.colours();
        let background = egui::Frame::NONE.fill(colours.surface);
        egui::CentralPanel::default().frame(background).show(ui, |ui| {
            let mut panes = Panes {
                graph: &mut self.graph,
                annotations: &mut self.annotations,
                resolved: &resolved,
                colours,
                runner: self.runner.as_ref(),
                canvas: &mut self.canvas,
                selection: &mut self.selection,
                open_windows: &open_windows,
                events: &mut events,
            };
            self.layout.ui(ui, &mut panes);
        });
        self.floating_windows(&ctx, &resolved, &mut events);
        for event in events {
            self.handle(&ctx, event);
        }
        self.create_menu(&ctx, &resolved);
        self.history.record(&self.graph, &self.annotations, &ctx);
        self.keep_running(&ctx);
    }
}

impl<N: NodeSet, A: AnnotationSet> eframe::App for App<N, A> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    /// Behind everything, in the theme's colours (eframe's default is near black whatever the theme).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.colours().surface.to_normalized_gamma_f32()
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, THEME_KEY, &self.themes[self.theme].name);
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
        self.redo.push(std::mem::replace(&mut self.current, previous));
        Some(self.restored())
    }

    fn redo(&mut self) -> Option<(Graph<N>, Annotations<A>)> {
        let next = self.redo.pop()?;
        self.undo.push(std::mem::replace(&mut self.current, next));
        Some(self.restored())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::Role;
    use crate::standard_components::themes::THEMES;
    use crate::{AnnotationTypes, Nodes};
    use docking::WindowState;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    /// The app on the example graph (which the tests don't save). Run with UPDATE_SNAPSHOTS=1 to accept changes; see
    /// tests/snapshots/.
    type TestApp = App<Nodes, AnnotationTypes>;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/rig.json")
    }

    fn harness() -> Harness<'static, TestApp> {
        let app = TestApp::open(Some(example()), THEMES);
        assert!(app.status.is_none(), "the example opens");
        let mut harness = Harness::builder()
            .with_size([1400.0, 850.0])
            .wgpu()
            .build_ui_state(|ui, app: &mut TestApp| app.show(ui), app);
        harness.run_steps(3);
        harness
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
                ("Settings", vec!["threshold", "camera"])
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
        pointer(&mut harness, start + egui::vec2(60.0, 120.0), None);
        pointer(&mut harness, start + egui::vec2(150.0, 300.0), None);
        harness.snapshot("graph_dragging_wire");

        // Dropped on empty canvas: offers what could go there, then adds and connects the choice.
        pointer(&mut harness, start + egui::vec2(150.0, 300.0), Some(false));
        assert!(harness.state().menu.is_some());
        harness.snapshot("graph_create_menu");
        let edges = harness.state().graph.edges().len();
        harness.get_by_label("Gain").click();
        harness.run_steps(2);
        let app = harness.state();
        assert_eq!(app.graph.edges().len(), edges + 1);
        let gain = Port { node: node_named(app, "Gain"), role: Role::Input, index: 0 };
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
        // Opening docks: the first window in a new panel below the canvas, the next beside it.
        app.layout.toggle(spikes);
        app.layout.toggle(lfp);
        app.layout.toggle(detector);
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
        assert_eq!(harness.query_all_by_label("Undock").count(), 1, "the docked windows' tab bar can undock");
        assert_eq!(harness.query_all_by_label("Dock").count(), 1, "the floating window can be docked");

        // Closing a docked window keeps its place for when it's reopened.
        let app = harness.state_mut();
        assert_eq!(app.layout.window(lfp), WindowState::Docked);
        assert_eq!(app.layout.window(detector), WindowState::Floating);
        app.layout.toggle(spikes);
        assert_eq!(app.layout.window(spikes), WindowState::Hidden);
        app.layout.toggle(spikes);
        assert_eq!(app.layout.window(spikes), WindowState::Docked);
        app.layout.dock(detector);
        assert_eq!(app.layout.window(detector), WindowState::Docked);
    }

    #[test]
    fn floating_window_moves_by_its_header() {
        let mut harness = harness();
        let detector = node_named(harness.state(), "Spike Detector");
        harness.state_mut().layout.toggle(detector);
        harness.state_mut().layout.float(detector);
        harness.run_steps(3);
        let area = |h: &Harness<TestApp>| h.ctx.memory(|m| m.area_rect(Id::unique(("node-window", detector))));
        let before = area(&harness).expect("window shown");
        // Grab the bar to the right of the tab, clear of the tab's close and dock buttons.
        let grab = egui::pos2(before.center().x + 30.0, before.top() + 12.0);
        pointer(&mut harness, grab, Some(true));
        pointer(&mut harness, grab + egui::vec2(20.0, 10.0), None);
        pointer(&mut harness, grab + egui::vec2(60.0, 40.0), None);
        pointer(&mut harness, grab + egui::vec2(60.0, 40.0), Some(false));
        let moved = area(&harness).unwrap().min - before.min;
        assert!(moved.x > 50.0 && moved.y > 30.0, "window should follow the drag, moved {moved:?}");
    }

    #[test]
    fn play_runs_the_graph() {
        let mut harness = harness();
        harness.get_by_label("Play (Space)").click();
        harness.run_steps(2);
        assert!(harness.state().runner.is_some());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let stats = harness.state().runner.as_ref().unwrap().stats();
        assert!(stats.completed > 0, "{stats:?}");
        // Editing a setting carries the run on with the new version of the graph.
        let first = harness.state().runner.as_ref().unwrap().graph().clone();
        let gain_free = node_named(harness.state(), "Spike Detector");
        if let Some(entry) = harness.state_mut().graph.get_mut(gain_free) {
            entry.node.set_setting(0, crate::standard_components::spikes::Threshold { sd: 6.0 }.into());
        }
        harness.run_steps(2);
        assert!(!Arc::ptr_eq(&first, &harness.state().runner.as_ref().unwrap().graph()));
        harness.key_press(Key::Space);
        harness.run_steps(2);
        assert!(harness.state().runner.is_none());
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
