//! People manage quick actions (see `crate::quick_action`) in Settings › Quick actions. Each is a file
//! (`name.quick-action.json`) in a folder beside the app's settings: the build's are copied there the first time (see
//! `give_built_ins`), and from then on are like any other. Quick actions can be made from the graph (the nodes
//! selected, or all of them), changed, copied to the clipboard and pasted in (to share them), and deleted. Any can be
//! left out of the basic view.
//!
//! The editor shows a quick action's nodes and edges as a graph, on a canvas of its own (the app's canvas, with
//! `canvas::Extras`): under each node, how it's found and what it's found as in the app's graph; each edge coloured
//! by what's done to it. Nodes are added as they are on the app's canvas (clicking it, or a port), or copied from the
//! nodes selected in the app's graph. Beside the canvas, the inspector changes what's selected: a node's rule, its
//! settings (taken from the node it's found as, if wanted) and its window; an edge's mode; or, with nothing selected,
//! the quick action's name, description, arrangement and base directory.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};

use super::Event;
use super::canvas::{Canvas, Selection};
use super::create_menu::{self, CreateMenu};
use super::dialog;
use super::help::info_icon;
use super::icons::Icon;
use super::toolbar;
use crate::annotation::{Annotations, AnnotationsEnum};
use crate::config_patching::PatchSets;
use crate::engine::{Edge, Graph, NodeId, NodesEnum};
use crate::layout::Layout;
use crate::layout::Side;
use crate::quick_action::{EdgeMode, Mode, Placement, QuickAction, QuickState, Rule, Window};
use crate::theme::Colours;

/// A file's a quick action if its name ends with this.
pub const EXTENSION: &str = ".quick-action.json";

/// A quick action, and its file: none only without a folder (in tests), for one the build ships.
pub struct Entry<N> {
    pub action: QuickAction<N>,
    pub file: Option<PathBuf>,
    /// When the file was last changed, if the file system says.
    pub modified: Option<std::time::SystemTime>,
}

/// The folder lists the build's quick actions it's been given in this file, by file name, so one deleted isn't given
/// again.
const GIVEN: &str = ".built-in";

/// The quick actions are files in a folder. The build's are copied there the first time the app finds them missing
/// (see `give_built_ins`), and from then on are people's own, to change or delete like any other.
pub struct QuickActions<N> {
    built_in: &'static [&'static str],
    /// The quick actions' files are in this folder, if there is one. Without one (in tests), the build's quick
    /// actions are read as they are, with no files.
    pub folder: Option<PathBuf>,
    pub entries: Vec<Entry<N>>,
    /// What couldn't be read, and why, a line for each.
    pub problems: Vec<String>,
}

impl<N> Default for QuickActions<N> {
    fn default() -> Self {
        QuickActions { built_in: &[], folder: None, entries: Vec::new(), problems: Vec::new() }
    }
}

impl<N: NodesEnum> QuickActions<N> {
    pub fn new(built_in: &'static [&'static str], folder: Option<PathBuf>, patches: &PatchSets) -> Self {
        let mut actions = QuickActions { built_in, folder, entries: Vec::new(), problems: Vec::new() };
        actions.reload(patches);
        actions
    }

    /// Reads the quick actions again, from the folder, in order of their files' names (after giving it the build's:
    /// see `give_built_ins`); or, without a folder, the build's.
    pub fn reload(&mut self, patches: &PatchSets) {
        self.entries.clear();
        self.problems.clear();
        let Some(folder) = &self.folder else {
            for text in self.built_in {
                match QuickAction::parse(text, patches) {
                    Ok(action) => self.entries.push(Entry { action, file: None, modified: None }),
                    Err(problem) => self.problems.push(format!("A built-in quick action: {problem}")),
                }
            }
            return;
        };
        if let Err(problem) = give_built_ins(folder, self.built_in) {
            self.problems.push(problem);
        }
        let Ok(dir) = std::fs::read_dir(folder) else { return };
        let mut files: Vec<PathBuf> = dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(EXTENSION)))
            .collect();
        files.sort();
        for file in files {
            let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let read = std::fs::read_to_string(&file).map_err(|e| e.to_string());
            let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
            match read.and_then(|text| QuickAction::parse(&text, patches)) {
                Ok(action) => self.entries.push(Entry { action, file: Some(file), modified }),
                Err(problem) => self.problems.push(format!("{name}: {problem}")),
            }
        }
    }

    /// Saves `action` to `file`, or (with none) to a new file in the folder, named for it; returns the file.
    pub fn save(&self, action: &QuickAction<N>, file: Option<&Path>, patches: &PatchSets) -> Result<PathBuf, String> {
        let file = match file {
            Some(file) => file.to_path_buf(),
            None => {
                let folder = self.folder.as_ref().ok_or("There's no folder for quick actions")?;
                std::fs::create_dir_all(folder).map_err(|e| format!("Can't make {}: {e}", folder.display()))?;
                new_file(folder, &action.name)
            }
        };
        std::fs::write(&file, action.to_text(patches)).map_err(|e| format!("Can't save {}: {e}", file.display()))?;
        Ok(file)
    }
}

/// The build's quick actions are copied into `folder` with this, as they are, each to a file named for it, unless the
/// folder's had it before (see `GIVEN`) or a file of that name is there already: a quick action changed, or one of the
/// same name, is left as it is.
fn give_built_ins(folder: &Path, built_in: &[&str]) -> Result<(), String> {
    let list = folder.join(GIVEN);
    let mut given: Vec<String> =
        std::fs::read_to_string(&list).unwrap_or_default().lines().map(str::to_string).collect();
    let mut changed = false;
    for text in built_in {
        let name = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v.get("name")?.as_str().map(str::to_string))
            .unwrap_or_default();
        let file = folder.join(format!("{}{EXTENSION}", slug(&name)));
        let file_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if given.contains(&file_name) {
            continue;
        }
        if !file.exists() {
            std::fs::create_dir_all(folder).map_err(|e| format!("Can't make {}: {e}", folder.display()))?;
            std::fs::write(&file, text).map_err(|e| format!("Can't save {}: {e}", file.display()))?;
        }
        given.push(file_name);
        changed = true;
    }
    if changed {
        std::fs::write(&list, given.join("\n") + "\n").map_err(|e| format!("Can't save {}: {e}", list.display()))?;
    }
    Ok(())
}

/// A quick action's name in lower case, a hyphen between words, for its file's name.
fn slug(name: &str) -> String {
    let words: Vec<String> =
        name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect();
    if words.is_empty() { "quick-action".to_string() } else { words.join("-") }
}

/// A new file in `folder` for a quick action named `name`: its name in lower case, a hyphen between words, and a
/// number after it if that's taken.
fn new_file(folder: &Path, name: &str) -> PathBuf {
    let stem = slug(name);
    let path = |n: usize| match n {
        1 => folder.join(format!("{stem}{EXTENSION}")),
        n => folder.join(format!("{stem}-{n}{EXTENSION}")),
    };
    (1..).map(path).find(|p| !p.exists()).unwrap_or_else(|| path(1))
}

/// The manager asks the app to do these.
pub enum Request {
    /// A new quick action from the nodes selected (or all, if none are).
    New,
    Edit(usize),
    Copy(usize),
    Delete(usize),
    Paste,
    OpenFolder,
}

/// Settings › Quick actions lists the quick actions, each offered in the basic view or not (`hidden`, by name), with
/// what can be done with each; and what couldn't be read. What's asked for is added to `requests`.
pub fn manager_ui<N: NodesEnum>(
    ui: &mut egui::Ui,
    actions: &QuickActions<N>,
    hidden: &mut BTreeSet<String>,
    selected: usize,
    colours: &Colours,
    requests: &mut Vec<Request>,
) {
    let weak = |text: &str| RichText::new(text).small().color(colours.text_secondary);
    ui.label(weak(
        "Quick actions are offered in the basic view, when they'd change something. Each is a file: share one by \
         copying the file, or with the copy button, then paste it in here.",
    ));
    if let Some(folder) = &actions.folder {
        ui.horizontal(|ui| {
            ui.label(weak(&format!("In {}", folder.display())));
            let open = toolbar::small_icon_button(ui, Icon::Open, "Open the folder", "Open the folder");
            if open.clicked() {
                requests.push(Request::OpenFolder);
            }
        });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let new = match selected {
            0 => "New from the graph".to_string(),
            1 => "New from the selected node".to_string(),
            n => format!("New from the {n} selected nodes"),
        };
        if ui.button(new).clicked() {
            requests.push(Request::New);
        }
        if ui.button("Paste").on_hover_text("A quick action copied to the clipboard").clicked() {
            requests.push(Request::Paste);
        }
    });
    ui.add_space(6.0);
    egui::Grid::new("quick actions").num_columns(3).min_col_width(0.0).spacing([10.0, 6.0]).striped(true).show(
        ui,
        |ui| {
            for (index, entry) in actions.entries.iter().enumerate() {
                let name = &entry.action.name;
                let mut on = !hidden.contains(name);
                let offered = ui.add(egui::Checkbox::without_text(&mut on)).on_hover_text("Offered in the basic view");
                let checked = on;
                let accessible = format!("Offer {name} in the basic view");
                offered.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::CheckBox, true, checked, accessible.clone())
                });
                if offered.changed() {
                    match on {
                        true => hidden.remove(name),
                        false => hidden.insert(name.clone()),
                    };
                }
                ui.vertical(|ui| {
                    ui.set_max_width(420.0);
                    ui.strong(name);
                    if !entry.action.description.is_empty() {
                        ui.label(weak(&entry.action.description));
                    }
                    if let Some(modified) = entry.modified {
                        let local: chrono::DateTime<chrono::Local> = modified.into();
                        ui.label(weak(&format!("Updated {}", local.format("%-d %b %Y, %H:%M"))));
                    }
                });
                ui.horizontal(|ui| {
                    if toolbar::small_icon_button(ui, Icon::Edit, "Edit", "Edit").clicked() {
                        requests.push(Request::Edit(index));
                    }
                    if toolbar::small_icon_button(ui, Icon::Copy, "Copy", "Copy to the clipboard, to share").clicked() {
                        requests.push(Request::Copy(index));
                    }
                    if entry.file.is_some()
                        && toolbar::small_icon_button(ui, Icon::Delete, "Delete", "Delete the file").clicked()
                    {
                        requests.push(Request::Delete(index));
                    }
                });
                ui.end_row();
            }
        },
    );
    for problem in &actions.problems {
        ui.label(RichText::new(problem).small().color(colours.warning));
    }
}

/// While a quick action's being changed, the app keeps it here: the quick action, with its nodes and edges as a graph
/// of their own, on a canvas, each node's rule and each edge's mode beside it.
pub struct Editor<N> {
    /// The quick action, but for its nodes and edges (see `action`).
    action: QuickAction<N>,
    /// The file it's saved to: none for a new one.
    pub file: Option<PathBuf>,
    graph: Graph<N>,
    rules: BTreeMap<NodeId, Rule>,
    modes: Vec<(Edge, EdgeMode)>,
    canvas: Canvas,
    selection: Selection,
    menu: Option<CreateMenu>,
}

/// Each frame, the editor says what came of it.
pub enum Outcome {
    Open,
    Cancelled,
    Save,
}

/// The editor's canvas draws, under each node, how it's found and what it's found as in the app's graph, and each edge
/// in a colour for its mode. Its menus change them.
struct Marks<'a> {
    rules: &'a mut BTreeMap<NodeId, Rule>,
    modes: &'a mut Vec<(Edge, EdgeMode)>,
    /// What each node's found as in the app's graph: none if the quick action wouldn't be offered.
    found: Option<BTreeMap<NodeId, String>>,
    removed: Vec<NodeId>,
    colours: &'a Colours,
}

impl crate::gui::canvas::Extras for Marks<'_> {
    fn app_graph(&self) -> bool {
        false
    }

    fn node_badge(&self, id: NodeId) -> Option<(String, egui::Color32)> {
        let rule = self.rules.get(&id).cloned().unwrap_or_default();
        let colour = match rule.mode {
            Mode::Require => self.colours.accent,
            Mode::Ensure => self.colours.success,
            Mode::IfPresent | Mode::Ignore => self.colours.text_secondary,
        };
        if rule.mode == Mode::Ignore {
            return Some((rule.mode.name().to_string(), colour));
        }
        let mut text = rule.mode.name().to_string();
        match rule.enabled {
            Some(true) => text.push_str(", enabled"),
            Some(false) => text.push_str(", disabled"),
            None => {}
        }
        if let Some(found) = &self.found {
            text.push('\n');
            text.push_str(&found.get(&id).cloned().unwrap_or_default());
        }
        Some((text, colour))
    }

    fn edge_colour(&self, edge: &Edge) -> Option<egui::Color32> {
        match self.modes.iter().find(|(e, _)| e == edge).map(|(_, mode)| *mode) {
            Some(EdgeMode::Replace) => Some(self.colours.warning),
            Some(EdgeMode::Remove) => Some(self.colours.error),
            _ => None,
        }
    }

    fn node_faded(&self, id: NodeId) -> bool {
        self.ignored(id)
    }

    fn edge_faded(&self, edge: &Edge) -> bool {
        edge_mode(self.modes, *edge) == EdgeMode::Ignore || self.ignored(edge.from.node) || self.ignored(edge.to.node)
    }

    fn node_menu(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let rule = self.rules.entry(id).or_default();
        let mut ignored = rule.mode == Mode::Ignore;
        if ui.checkbox(&mut ignored, Mode::Ignore.choice()).changed() {
            rule.mode = if ignored { Mode::Ignore } else { Mode::Ensure };
        }
        if !ignored {
            ui.separator();
            ui.weak("When not found");
            for mode in [Mode::Ensure, Mode::IfPresent, Mode::Require] {
                ui.radio_value(&mut rule.mode, mode, mode.choice());
            }
        }
        ui.separator();
        if ui.button("Remove from the quick action").clicked() {
            self.removed.push(id);
            ui.close();
        }
    }

    fn edge_menu(&mut self, ui: &mut egui::Ui, edge: Edge) {
        let mut mode = edge_mode(self.modes, edge);
        for choice in EdgeMode::ALL {
            ui.radio_value(&mut mode, choice, choice.name());
        }
        set_edge_mode(self.modes, edge, mode);
    }
}

impl Marks<'_> {
    fn ignored(&self, id: NodeId) -> bool {
        self.rules.get(&id).is_some_and(|r| r.mode == Mode::Ignore)
    }
}

fn edge_mode(modes: &[(Edge, EdgeMode)], edge: Edge) -> EdgeMode {
    modes.iter().find(|(e, _)| *e == edge).map_or(EdgeMode::Ensure, |(_, mode)| *mode)
}

fn set_edge_mode(modes: &mut Vec<(Edge, EdgeMode)>, edge: Edge, mode: EdgeMode) {
    modes.retain(|(e, _)| *e != edge);
    if mode != EdgeMode::Ensure {
        modes.push((edge, mode));
    }
}

/// The inspector, beside the canvas, is this wide.
const INSPECTOR: f32 = 330.0;

impl<N: NodesEnum> Editor<N> {
    pub fn new(action: QuickAction<N>, file: Option<PathBuf>, colours: &'static Colours) -> Self {
        let (graph, modes) = action.graph();
        let rules = action.roles.iter().map(|r| (r.id, r.rule.clone())).collect();
        let mut canvas = Canvas::new(colours);
        canvas.legend_open = false;
        canvas.id_salt = Some(egui::Id::unique("quick action canvas"));
        Editor { action, file, graph, rules, modes, canvas, selection: Selection::default(), menu: None }
    }

    /// The quick action as it is now.
    pub fn action(&self) -> QuickAction<N> {
        let mut action = self.action.clone();
        action.set_graph(&self.graph, &self.rules, &self.modes);
        action
    }

    #[cfg(test)]
    pub fn name(&self) -> &str {
        &self.action.name
    }

    /// Tests find a node of the quick action by its label with this.
    #[cfg(test)]
    pub fn role_named(&self, label: &str) -> NodeId {
        self.graph.iter().find(|(_, e)| e.label == label).map(|(id, _)| id).unwrap()
    }

    /// Tests select a node of the quick action with this.
    #[cfg(test)]
    pub fn select(&mut self, id: NodeId) {
        self.selection.only(id);
    }

    /// The editor takes most of the app's window, as a modal: the canvas on the left, and on the right, what's
    /// selected (a node, an edge) or, with nothing selected, the quick action itself. `graph` is the app's, which
    /// the quick action would be done on, and whose `selected` nodes can be added to it.
    pub fn show<A: AnnotationsEnum>(
        &mut self,
        ctx: &egui::Context,
        graph: &Graph<N>,
        layout: &Layout,
        selected: &BTreeSet<NodeId>,
        base_directories: &[&'static str],
        colours: &'static Colours,
    ) -> Outcome {
        let mut outcome = Outcome::Open;
        let action = self.action();
        let state = QuickState { graph, layout };
        let found = action.find(graph);
        let preview = match &found {
            Err(label) => format!("Not offered now: there's no {label}"),
            Ok(_) => match action.run(&state) {
                Some(change) => format!("Now, it would have {}", change.summary),
                None => "Not offered now: it would change nothing".into(),
            },
        };
        let found_as: Option<BTreeMap<NodeId, String>> = found.as_ref().ok().map(|found| {
            (self.graph.ids())
                .map(|role| {
                    let mode = self.rules.get(&role).map(|r| r.mode).unwrap_or_default();
                    let label = found.get(&role).and_then(|id| graph.get(*id)).map(|e| e.label.as_str());
                    let text = match (label, mode) {
                        (_, Mode::Ignore) => String::new(),
                        (Some(label), _) => format!("Is {label}"),
                        (None, Mode::Ensure) => "Not in the graph: added".into(),
                        (None, _) => "Not in the graph".into(),
                    };
                    (role, text)
                })
                .collect()
        });
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).max(700.0);
        // Room for the title bar, the footer and the margins, which are about this tall together.
        let height = (screen.height() * 0.92 - 190.0).max(300.0);
        let named = !self.action.name.trim().is_empty();
        let body = |ui: &mut egui::Ui| {
            let size = egui::vec2(ui.available_width(), height);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.set_height(height);
                let canvas_size = egui::vec2(size.x - INSPECTOR - 16.0, height);
                ui.allocate_ui(canvas_size, |ui| {
                    ui.set_min_size(canvas_size);
                    ui.set_max_size(canvas_size);
                    egui::Frame::new().stroke(egui::Stroke::new(1.0, colours.border)).show(ui, |ui| {
                        self.canvas_ui::<A>(ui, found_as.clone(), colours);
                    });
                });
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_width(INSPECTOR);
                    let scroll = egui::ScrollArea::vertical().max_height(height).auto_shrink([false, false]);
                    scroll.id_salt("inspector").show(ui, |ui| {
                        self.inspector(ui, graph, layout, &found, selected, base_directories);
                    });
                });
            });
        };
        let footer = |ui: &mut egui::Ui| {
            ui.label(RichText::new(&preview).small().color(colours.text_secondary));
            let (save, cancel) = dialog::at_right(ui, |ui| {
                let save = ui.add_enabled(named, egui::Button::new("Save")).clicked();
                (save, ui.button("Cancel").clicked())
            });
            if save {
                outcome = Outcome::Save;
            } else if cancel {
                outcome = Outcome::Cancelled;
            }
        };
        let closed = dialog::show(ctx, "Quick action", width, colours, body, footer);
        if closed && matches!(outcome, Outcome::Open) && self.menu.is_none() {
            outcome = Outcome::Cancelled;
        }
        self.create_menu(ctx);
        outcome
    }

    fn canvas_ui<A: AnnotationsEnum>(
        &mut self,
        ui: &mut egui::Ui,
        found: Option<BTreeMap<NodeId, String>>,
        colours: &Colours,
    ) {
        let resolved = self.graph.resolve();
        let mut annotations = Annotations::<A>::default();
        let mut events = Vec::new();
        let mut marks = Marks { rules: &mut self.rules, modes: &mut self.modes, found, removed: Vec::new(), colours };
        self.canvas.show(
            ui,
            &mut self.graph,
            &mut annotations,
            &resolved,
            &mut self.selection,
            &HashSet::new(),
            &mut events,
            &mut marks,
        );
        for id in std::mem::take(&mut marks.removed) {
            self.remove(id);
        }
        for event in events {
            match event {
                Event::OpenMenu { screen, scene, from } => {
                    self.menu = Some(CreateMenu::new(screen, scene, from).above_modal());
                }
                Event::DeleteEdge(edge) => self.graph.disconnect(edge),
                _ => {}
            }
        }
        let deleting = ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace));
        if deleting && !ui.ctx().egui_wants_keyboard_input() {
            for id in std::mem::take(&mut self.selection.nodes) {
                self.remove(id);
            }
            if let Some(edge) = self.selection.edge.take() {
                self.graph.disconnect(edge);
            }
        }
        // Edges gone (disconnected on the canvas, or with their nodes) take their modes with them.
        let edges = self.graph.edges();
        self.modes.retain(|(e, _)| edges.contains(e));
    }

    fn remove(&mut self, id: NodeId) {
        self.graph.remove(id);
        self.rules.remove(&id);
        self.selection.nodes.remove(&id);
    }

    /// Clicking the canvas, or a port, opens the menu to add a node; what's chosen is added to the quick action.
    fn create_menu(&mut self, ctx: &egui::Context) {
        let Some(menu) = &mut self.menu else { return };
        let resolved = self.graph.resolve();
        let (choice, keep) = menu.show(ctx, &self.graph, &resolved, &BTreeSet::new());
        let (scene, from) = (menu.scene, menu.from);
        if !keep {
            self.menu = None;
        }
        if let Some(choice) = choice {
            let (added, _) = create_menu::add_chosen(&mut self.graph, &resolved, choice, scene, from);
            if let Some(id) = added {
                self.selection.only(id);
            }
        }
    }

    /// Nodes selected in the app's graph are added to the quick action with this, with the edges between them, to
    /// the right of what's there.
    fn add_from(&mut self, graph: &Graph<N>, selected: &BTreeSet<NodeId>) {
        let right = self.graph.iter().map(|(_, e)| e.pos[0] + 280.0).fold(f32::NEG_INFINITY, f32::max);
        let left = selected.iter().filter_map(|id| graph.get(*id)).map(|e| e.pos[0]).fold(f32::INFINITY, f32::min);
        let shift = if right.is_finite() { right - left } else { 0.0 };
        let mut new_ids = BTreeMap::new();
        for &id in selected {
            let Some(entry) = graph.get(id) else { continue };
            let added = self.graph.add(entry.node.clone(), [entry.pos[0] + shift, entry.pos[1]]);
            if let Some(new) = self.graph.get_mut(added) {
                new.label = entry.label.clone();
                new.disabled = entry.disabled;
                new.show_in_basic_override = entry.show_in_basic_override;
            }
            new_ids.insert(id, added);
        }
        for edge in graph.edges() {
            if let (Some(from), Some(to)) = (new_ids.get(&edge.from.node), new_ids.get(&edge.to.node)) {
                let from = crate::engine::PortAddress { node: *from, ..edge.from };
                let to = crate::engine::PortAddress { node: *to, ..edge.to };
                let _ = self.graph.connect(from, to);
            }
        }
        self.selection.clear();
        self.selection.nodes.extend(new_ids.values());
        self.canvas.frame_all();
    }

    #[allow(clippy::too_many_arguments)]
    fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        graph: &Graph<N>,
        layout: &Layout,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
        selected: &BTreeSet<NodeId>,
        base_directories: &[&'static str],
    ) {
        if let Some(edge) = self.selection.edge {
            let name = |port: crate::engine::PortAddress| {
                let label = self.graph.get(port.node).map_or("?", |e| e.label.as_str());
                let ids = self.graph.get(port.node).map(|e| e.node.port_ids(port.role)).unwrap_or_default();
                format!("{label} › {}", ids.get(port.port_idx).copied().unwrap_or("?").replace('_', " "))
            };
            ui.strong("Connection");
            ui.label(format!("From {}\nto {}", name(edge.from), name(edge.to)));
            ui.add_space(6.0);
            let mut mode = edge_mode(&self.modes, edge);
            for choice in EdgeMode::ALL {
                radio_info(ui, &mut mode, choice, choice.name(), edge_info(choice));
            }
            set_edge_mode(&mut self.modes, edge, mode);
            return;
        }
        if let Some(id) = self.selection.single() {
            return self.node_inspector(ui, id, graph, found);
        }
        self.action_inspector(ui, graph, layout, found, selected, base_directories);
    }

    fn node_inspector(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        graph: &Graph<N>,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
    ) {
        let others: Vec<(NodeId, String)> =
            self.graph.iter().filter(|(o, _)| *o != id).map(|(o, e)| (o, e.label.clone())).collect();
        let Some(entry) = self.graph.get_mut(id) else { return };
        let rule = self.rules.entry(id).or_default();
        let kind = entry.node.info().name;
        ui.strong(&entry.label);
        let label = ui.label("Label");
        ui.text_edit_singleline(&mut entry.label).labelled_by(label.id);
        let mut ignored = rule.mode == Mode::Ignore;
        ui.horizontal(|ui| {
            if ui.checkbox(&mut ignored, "Ignore this node").changed() {
                rule.mode = if ignored { Mode::Ignore } else { Mode::Ensure };
            }
            info_icon(
                ui,
                "An ignored node stays in the quick action only to show what surrounds the rest. The graph is never \
                 searched for it, and neither the node nor its connections are changed.",
            );
        });
        if ignored {
            return;
        }
        ui.add_space(8.0);

        ui.strong("Matching");
        ui.horizontal(|ui| {
            ui.label(format!("Type: {kind}"));
            info_icon(ui, &format!("Only {kind} nodes in the graph are considered."));
        });
        let row = ui.horizontal(|ui| {
            let label = ui.label("Label pattern");
            info_icon(
                ui,
                "A regular expression, such as ^Band-pass or tetrode \\d, matched against anywhere in the label. \
                 Empty, any label matches.",
            );
            label
        });
        let mut pattern = rule.label_pattern.clone().unwrap_or_default();
        let edit = ui.add(egui::TextEdit::singleline(&mut pattern).hint_text("Any label")).labelled_by(row.inner.id);
        if edit.changed() {
            rule.label_pattern = (!pattern.is_empty()).then_some(pattern.clone());
        }
        if !pattern.is_empty()
            && let Err(e) = regex::Regex::new(&pattern)
        {
            let error = e.to_string();
            let line = error.lines().last().unwrap_or("").trim().to_string();
            ui.label(RichText::new(format!("Not a valid pattern: {line}")).small().color(ui.visuals().error_fg_color));
        }
        let found_as = found.as_ref().ok().and_then(|f| f.get(&id).copied()).and_then(|other| graph.get(other));
        let now = match (found_as, found) {
            (Some(other), _) => format!("In the graph now: {}", other.label),
            (None, Ok(_)) => "In the graph now: not found".to_string(),
            (None, Err(_)) => "In the graph now: the quick action isn't offered".to_string(),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(now).weak());
            info_icon(
                ui,
                "Of the nodes that match, the left-most on the canvas is used (of two equally far left, the higher). \
                 A node already used for another of the quick action's nodes is passed over.",
            );
        });
        ui.add_space(8.0);

        ui.strong("When found");
        let choices = [
            (
                None,
                "Leave enabled or disabled",
                "A node created when none is found is enabled or disabled as this one is.",
            ),
            (Some(true), "Enable", "A node created when none is found is enabled too."),
            (Some(false), "Disable", "A node created when none is found is disabled too."),
        ];
        for (choice, text, info) in choices {
            radio_info(ui, &mut rule.enabled, choice, text, info);
        }
        ui.horizontal(|ui| {
            ui.label("Apply these settings");
            info_icon(
                ui,
                "Each setting ticked is copied from this node onto the node found. A node added has all of this \
                 node's settings, ticked or not.",
            );
        });
        let settings = serde_json::to_value(&entry.node).ok().and_then(|v| v.get("settings").cloned());
        let keys: Vec<String> =
            settings.as_ref().and_then(|s| s.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        for key in &keys {
            let mut on = rule.settings.contains(key);
            if ui.checkbox(&mut on, key.replace('_', " ")).changed() {
                match on {
                    true => rule.settings.push(key.clone()),
                    false => rule.settings.retain(|k| k != key),
                }
            }
        }
        if let Some(other) = found_as {
            ui.horizontal(|ui| {
                if ui.button(format!("Take the settings of {}", other.label)).clicked() {
                    entry.node = other.node.clone();
                }
                info_icon(
                    ui,
                    &format!(
                        "In the graph now, {} is the node found for this one. Its current settings replace this \
                         node's.",
                        other.label
                    ),
                );
            });
        }
        ui.add_space(8.0);

        ui.strong("When not found");
        for mode in [Mode::Ensure, Mode::IfPresent, Mode::Require] {
            radio_info(ui, &mut rule.mode, mode, mode.choice(), not_found_info(mode));
        }
        ui.add_space(8.0);

        window_ui(ui, &mut rule.window, &others);
    }

    fn action_inspector(
        &mut self,
        ui: &mut egui::Ui,
        graph: &Graph<N>,
        layout: &Layout,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
        selected: &BTreeSet<NodeId>,
        base_directories: &[&'static str],
    ) {
        ui.strong("Quick action");
        let action = &mut self.action;
        let name = ui.label("Name");
        ui.text_edit_singleline(&mut action.name).labelled_by(name.id);
        let description = ui.label("Description");
        ui.add(egui::TextEdit::multiline(&mut action.description).desired_rows(3)).labelled_by(description.id);
        ui.add_space(8.0);
        let add = match selected.len() {
            0 => None,
            1 => Some("Add the node selected in the graph".to_string()),
            n => Some(format!("Add the {n} nodes selected in the graph")),
        };
        ui.horizontal(|ui| {
            if let Some(add) = add
                && ui.button(add).clicked()
            {
                self.add_from(graph, selected);
            }
            if ui.button("Arrange").on_hover_text("Lay out the quick action's nodes afresh").clicked() {
                for (id, pos) in super::arrange::arrange(&self.graph) {
                    if let Some(entry) = self.graph.get_mut(id) {
                        entry.pos = pos;
                    }
                }
                self.canvas.frame_all();
            }
            info_icon(
                ui,
                "On the canvas, selecting a node or a connection shows how the quick action treats it, and \
                 right-clicking one opens its menu. Clicking the canvas, or a port, adds a node, and Delete takes \
                 what's selected out of the quick action. To keep part of the graph here for context, without the \
                 quick action touching it, choose Ignore.",
            );
        });
        ui.add_space(8.0);

        ui.strong("Panes");
        let action = &mut self.action;
        let mut arrange = action.panes.arrangement.is_some();
        let ticked = ui
            .horizontal(|ui| {
                let ticked = ui.checkbox(&mut arrange, "Arrange as they are now");
                info_icon(
                    ui,
                    "When the quick action runs, home and the windows of the nodes found are laid out as the app has \
                     them now.",
                );
                ticked
            })
            .inner;
        let again = arrange && ui.small_button("As they are now, again").clicked();
        if ticked.changed() && !arrange {
            action.panes.arrangement = None;
        } else if ticked.changed() || again {
            let ids: Vec<(NodeId, NodeId)> =
                (found.iter()).flat_map(|f| f.iter()).map(|(role, id)| (*id, *role)).collect();
            action.panes.arrangement = layout
                .arrangement()
                .and_then(|a| a.map_windows(&|id| ids.iter().find(|(node, _)| *node == id).map(|p| p.1)));
        }
        ui.add_space(8.0);
        let label = ui.horizontal(|ui| {
            let label = ui.label("Base directory");
            info_icon(ui, "Chosen when the quick action runs, unless a recording is under way.");
            label
        });
        let current = action.base_directory.clone().unwrap_or_else(|| "As it is".into());
        egui::ComboBox::from_id_salt("base directory")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut action.base_directory, None, "As it is");
                for name in base_directories {
                    ui.selectable_value(&mut action.base_directory, Some(name.to_string()), *name);
                }
            })
            .response
            .labelled_by(label.inner.id);
        ui.horizontal(|ui| {
            ui.checkbox(&mut action.arrange_if_empty, "Lay out the graph afresh");
            info_icon(ui, "Only when the quick action builds the graph from nothing.");
        });
    }
}

/// A node's window, if open, is left open or closed; if not, left closed or opened, going (the first time) where
/// `Placement` says, relative to the window of one of `others` (the quick action's other nodes, with their labels) for
/// some.
fn window_ui(ui: &mut egui::Ui, window: &mut Window, others: &[(NodeId, String)]) {
    ui.strong("If the node's window is open");
    radio_info(ui, &mut window.close, false, "Leave it open", "The window stays where it is, as it is.");
    radio_info(ui, &mut window.close, true, "Close it", "A docked window keeps its place for when it's next opened.");
    ui.add_space(8.0);

    ui.strong("If the node's window isn't open");
    let opening = window.open.is_some();
    radio_info(ui, &mut window.open, None, "Leave it closed", "The window stays closed.");
    let open = ui.horizontal(|ui| {
        let open = ui.radio(opening, "Open it").clicked();
        info_icon(
            ui,
            "A window that's been open before opens where it was; one opening for the first time goes where chosen \
             below.",
        );
        open
    });
    if open.inner && !opening {
        window.open = Some(Placement::Default);
    }
    let Some(placement) = &mut window.open else { return };

    let first_other = others.first().map(|(o, _)| *o);
    let kinds: Vec<(&str, Option<Placement>)> = vec![
        ("Where a new window goes", Some(Placement::Default)),
        ("In front, in the largest group", Some(Placement::Largest)),
        ("Behind another window", first_other.map(Placement::Behind)),
        ("Next to another window", first_other.map(|of| Placement::Beside { of, side: Side::Right, share: 0.5 })),
    ];
    let kind = |p: &Placement| match p {
        Placement::Default => 0,
        Placement::Largest => 1,
        Placement::Behind(_) => 2,
        Placement::Beside { .. } => 3,
    };
    let current = kind(placement);
    let label = ui.horizontal(|ui| {
        let label = ui.label("Where, the first time");
        info_icon(
            ui,
            "• Where a new window goes: as the app places any window opened for the first time.\n\
             • In front, in the largest group: as the front tab of the largest group of tabs.\n\
             • Behind another window: as a tab behind the chosen node's window.\n\
             • Next to another window: in a group of its own on one side of the chosen node's window, taking a \
             share of the space.\n\n\
             Placed relative to a window that isn't open, the window goes where a new window goes.",
        );
        label
    });
    egui::ComboBox::from_id_salt("placement")
        .selected_text(kinds[current].0)
        .width(INSPECTOR - 20.0)
        .show_ui(ui, |ui| {
            for (i, (name, choice)) in kinds.iter().enumerate() {
                let Some(choice) = choice else { continue };
                if ui.selectable_label(i == current, *name).clicked() && i != current {
                    *placement = *choice;
                }
            }
        })
        .response
        .labelled_by(label.inner.id);

    let name = |of: NodeId| others.iter().find(|(o, _)| *o == of).map_or("?", |(_, l)| l.as_str()).to_string();
    let which = |ui: &mut egui::Ui, of: &mut NodeId| {
        let label = ui.label("Whose window");
        egui::ComboBox::from_id_salt("anchor")
            .selected_text(name(*of))
            .width(INSPECTOR - 20.0)
            .show_ui(ui, |ui| {
                for (other, label) in others {
                    ui.selectable_value(of, *other, label);
                }
            })
            .response
            .labelled_by(label.id);
    };
    match placement {
        Placement::Behind(of) => which(ui, of),
        Placement::Beside { of, side, share } => {
            which(ui, of);
            ui.horizontal(|ui| {
                let sides = [
                    (Side::Right, "on its right"),
                    (Side::Left, "on its left"),
                    (Side::Above, "above it"),
                    (Side::Below, "below it"),
                ];
                let side_name = sides.iter().find(|(s, _)| s == side).map_or("", |(_, n)| *n);
                let label = ui.label("Side");
                egui::ComboBox::from_id_salt("side")
                    .selected_text(side_name)
                    .show_ui(ui, |ui| {
                        for (choice, text) in sides {
                            ui.selectable_value(side, choice, text);
                        }
                    })
                    .response
                    .labelled_by(label.id);
                let shares = [(1.0 / 3.0, "a third"), (0.5, "half"), (2.0 / 3.0, "two thirds")];
                let share_name = (shares.iter())
                    .min_by(|a, b| (a.0 - *share).abs().total_cmp(&(b.0 - *share).abs()))
                    .map_or("", |(_, n)| *n);
                let label = ui.label("taking");
                egui::ComboBox::from_id_salt("share")
                    .selected_text(share_name)
                    .show_ui(ui, |ui| {
                        for (choice, text) in shares {
                            if ui.selectable_label(share_name == text, text).clicked() {
                                *share = choice;
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id);
            });
        }
        Placement::Default | Placement::Largest => {}
    }
}

/// A choice in the inspector, with an ⓘ saying what it does.
fn radio_info<T: PartialEq>(ui: &mut egui::Ui, current: &mut T, choice: T, text: &str, info: &str) {
    ui.horizontal(|ui| {
        ui.radio_value(current, choice, text);
        info_icon(ui, info);
    });
}

fn not_found_info(mode: Mode) -> &'static str {
    match mode {
        Mode::Ensure => {
            "With no node in the graph to match this one, a node like this one is created, with all of its settings, \
             ticked or not."
        }
        Mode::IfPresent => {
            "With no node in the graph to match this one, the quick action still runs, leaving out this node's \
             connections and window."
        }
        Mode::Require => {
            "With no node in the graph to match this one, the quick action isn't offered. Use VR computer, for \
             example, isn't offered without a position detector."
        }
        Mode::Ignore => "",
    }
}

fn edge_info(mode: EdgeMode) -> &'static str {
    match mode {
        EdgeMode::Ensure => "Connected only if the input (or a control's reading end) has no connection yet.",
        EdgeMode::Replace => "Connected, replacing any connection to the input.",
        EdgeMode::Remove => "Removed, if the graph has it.",
        EdgeMode::Ignore => {
            "Left as the graph has it. An ignored connection stays here only to show what surrounds the rest."
        }
    }
}

/// The app opens the quick actions' folder in the system's file manager with this.
pub fn open_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(folder).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_are_named_for_the_quick_action() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-quick-actions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = new_file(&dir, "Use VR computer!");
        assert_eq!(first.file_name().unwrap(), "use-vr-computer.quick-action.json");
        std::fs::write(&first, "").unwrap();
        assert_eq!(new_file(&dir, "Use VR computer").file_name().unwrap(), "use-vr-computer-2.quick-action.json");
        assert_eq!(new_file(&dir, "…").file_name().unwrap(), "quick-action.quick-action.json");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_builds_quick_actions_are_given_to_the_folder_once() {
        let dir = std::env::temp_dir().join(format!("ephys-rs-given-quick-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let built_in = [r#"{"name": "Use VR computer!", "graph": {"nodes": {}}}"#];
        give_built_ins(&dir, &built_in).unwrap();
        let file = dir.join("use-vr-computer.quick-action.json");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), built_in[0], "as it is");
        // Changed, the file's left as it is; deleted, it isn't given again.
        std::fs::write(&file, "changed").unwrap();
        give_built_ins(&dir, &built_in).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed");
        std::fs::remove_file(&file).unwrap();
        give_built_ins(&dir, &built_in).unwrap();
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
