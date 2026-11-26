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

use super::canvas::{Canvas, Selection};
use super::dialog;
use super::help::{info_icon, info_icon_with, regex_cheat_sheet};
use super::icons::Icon;
use super::style;
use super::toolbar;
use crate::annotation::{Annotations, AnnotationsEnum};
use crate::config_patching::PatchSets;
use crate::engine::{Edge, Graph, NodeId, NodesEnum};
use crate::layout::Layout;
use crate::layout::Side;
use crate::quick_action::{ANY_LABEL, EdgeRule, Mode, Placement, QuickAction, Rule, Window};
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
    // The names and descriptions wrap to leave room for the checkbox before them and the buttons after, so the list is
    // no wider than the settings window's other sections.
    let text_width = (ui.available_width() - 32.0 - 96.0).max(120.0);
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
                    ui.set_max_width(text_width);
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
    edge_rules: Vec<(Edge, EdgeRule)>,
    canvas: Canvas,
    selection: Selection,
    /// The nodes whose settings are applied, chosen in the inspector before any are ticked (a rule that applies none
    /// leaves them as they are, so this keeps the choice while none are).
    applying: BTreeSet<NodeId>,
}

/// Each frame, the editor says what came of it.
pub enum Outcome {
    Open,
    Cancelled,
    Save,
}

/// The editor's canvas draws, under each node, what the quick action does with it (coloured for what happens if it isn't
/// found), and each edge in a colour for its mode. Its menus change them.
struct Marks<'a> {
    rules: &'a mut BTreeMap<NodeId, Rule>,
    edge_rules: &'a mut Vec<(Edge, EdgeRule)>,
    colours: &'a Colours,
}

impl crate::gui::canvas::Extras for Marks<'_> {
    fn app_graph(&self) -> bool {
        false
    }

    fn rewiring(&self) -> bool {
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
            return None;
        }
        // In the inspector's words, leaving out what does nothing (but for what happens if it isn't found).
        let mut lines = Vec::new();
        let mut if_found = Vec::new();
        match rule.enabled {
            Some(true) => if_found.push("enable"),
            Some(false) => if_found.push("disable"),
            None => {}
        }
        if !rule.settings.is_empty() {
            if_found.push("apply settings");
        }
        if !if_found.is_empty() {
            lines.push(format!("If found: {}", if_found.join(", ")));
        }
        let otherwise = match rule.mode {
            Mode::Ensure => "create the node",
            Mode::Require => "don't offer the quick action",
            Mode::IfPresent | Mode::Ignore => "do nothing",
        };
        // "Else" follows an "If found"; on its own, it says its case.
        let case = if lines.is_empty() { "If not found" } else { "Else" };
        lines.push(format!("{case}: {otherwise}"));
        let mut window = Vec::new();
        if rule.window.close {
            window.push("close it if open");
        }
        if rule.window.open.is_some() {
            window.push("open it if not");
        }
        if !window.is_empty() {
            lines.push(format!("Window: {}", window.join(", ")));
        }
        Some((lines.join("\n"), colour))
    }

    fn edge_colour(&self, edge: &Edge) -> Option<egui::Color32> {
        let rule = edge_rule(self.edge_rules, *edge);
        if rule.replace { Some(self.colours.warning) } else { None }
    }

    fn node_faded(&self, id: NodeId) -> bool {
        self.ignored(id)
    }

    fn edge_faded(&self, edge: &Edge) -> bool {
        edge_rule(self.edge_rules, *edge).does_nothing() || self.ignored(edge.from.node) || self.ignored(edge.to.node)
    }

    fn node_menu(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let rule = self.rules.entry(id).or_default();
        let mut used = rule.mode != Mode::Ignore;
        if ui.checkbox(&mut used, "Use this node").changed() {
            rule.mode = if used { Mode::Ensure } else { Mode::Ignore };
        }
        if used {
            ui.separator();
            ui.weak("When not found");
            for mode in [Mode::IfPresent, Mode::Ensure, Mode::Require] {
                ui.radio_value(&mut rule.mode, mode, mode.choice());
            }
        }
    }

    fn edge_menu(&mut self, ui: &mut egui::Ui, edge: Edge) {
        let mut rule = edge_rule(self.edge_rules, edge);
        let mut used = !rule.does_nothing();
        if ui.checkbox(&mut used, "Use this connection").changed() {
            rule = if used { EdgeRule::default() } else { EdgeRule::NOTHING };
        }
        if used {
            ui.separator();
            ui.checkbox(&mut rule.connect, "If free: connect them");
            ui.checkbox(&mut rule.replace, "If occupied: connect them, replacing that");
        }
        set_edge_rule(self.edge_rules, edge, rule);
    }
}

impl Marks<'_> {
    fn ignored(&self, id: NodeId) -> bool {
        self.rules.get(&id).is_some_and(|r| r.mode == Mode::Ignore)
    }
}

fn edge_rule(rules: &[(Edge, EdgeRule)], edge: Edge) -> EdgeRule {
    rules.iter().find(|(e, _)| *e == edge).map_or(EdgeRule::default(), |(_, rule)| *rule)
}

fn set_edge_rule(rules: &mut Vec<(Edge, EdgeRule)>, edge: Edge, rule: EdgeRule) {
    rules.retain(|(e, _)| *e != edge);
    if rule != EdgeRule::default() {
        rules.push((edge, rule));
    }
}

/// The inspector, beside the canvas, is this wide.
const INSPECTOR: f32 = 330.0;

impl<N: NodesEnum> Editor<N> {
    pub fn new(action: QuickAction<N>, file: Option<PathBuf>, colours: &'static Colours) -> Self {
        let (graph, edge_rules) = action.graph();
        let rules = action.roles.iter().map(|r| (r.id, r.rule.clone())).collect();
        let mut canvas = Canvas::new(colours);
        canvas.legend_open = false;
        canvas.id_salt = Some(egui::Id::unique("quick action canvas"));
        let applying = action.roles.iter().filter(|r| !r.rule.settings.is_empty()).map(|r| r.id).collect();
        Editor { action, file, graph, rules, edge_rules, canvas, selection: Selection::default(), applying }
    }

    /// The quick action as it is now.
    pub fn action(&self) -> QuickAction<N> {
        let mut action = self.action.clone();
        action.set_graph(&self.graph, &self.rules, &self.edge_rules);
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

    /// Tests select an edge of the quick action, into `to`, with this.
    #[cfg(test)]
    pub fn select_edge_into(&mut self, to: NodeId) {
        self.selection.clear();
        self.selection.edge = self.graph.edges().iter().find(|e| e.to.node == to).copied();
    }

    /// The editor takes most of the app's window, as a modal: the canvas on the left, and on the right, what's
    /// selected (a node, an edge) or, with nothing selected, the quick action itself. `graph` is the app's, which
    /// the quick action would be done on.
    pub fn show<A: AnnotationsEnum>(
        &mut self,
        ctx: &egui::Context,
        graph: &Graph<N>,
        layout: &Layout,
        base_directories: &[&'static str],
        colours: &'static Colours,
    ) -> Outcome {
        let mut outcome = Outcome::Open;
        let found = self.action().find(graph);
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
                        self.canvas_ui::<A>(ui, colours);
                    });
                });
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_width(INSPECTOR);
                    ui.set_height(height);
                    self.inspector(ui, graph, layout, &found, base_directories, colours);
                });
            });
        };
        let footer = |ui: &mut egui::Ui| {
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
        if closed && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }

    fn canvas_ui<A: AnnotationsEnum>(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let resolved = self.graph.resolve();
        let mut annotations = Annotations::<A>::default();
        let mut events = Vec::new();
        let mut marks = Marks { rules: &mut self.rules, edge_rules: &mut self.edge_rules, colours };
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
    }

    fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        graph: &Graph<N>,
        layout: &Layout,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
        base_directories: &[&'static str],
        colours: &Colours,
    ) {
        if let Some(edge) = self.selection.edge {
            return self.edge_inspector(ui, edge, colours);
        }
        if let Some(id) = self.selection.single() {
            return self.node_inspector(ui, id, graph, found, colours);
        }
        self.action_inspector(ui, layout, found, base_directories, colours);
    }

    /// A connection's inspector: its two ends, as a title, and what's done to it in each case.
    fn edge_inspector(&mut self, ui: &mut egui::Ui, edge: Edge, colours: &Colours) {
        let end = |port: crate::engine::PortAddress| {
            let entry = self.graph.get(port.node);
            let label = entry.map_or_else(|| "?".to_string(), |e| e.label.clone());
            let colour = entry.map_or(colours.text_secondary, |e| style::category_colour(e.node.info().category));
            let ids = entry.map(|e| e.node.port_ids(port.role)).unwrap_or_default();
            let port = ids.get(port.port_idx).copied().unwrap_or("?").replace('_', " ");
            (label, colour, port)
        };
        let (from, to) = (end(edge.from), end(edge.to));
        let resolved = self.graph.resolve();
        let mut rule = edge_rule(&self.edge_rules, edge);
        let mut used = !rule.does_nothing();
        let paint = |painter: &egui::Painter, start, finish| {
            super::canvas::paint_edge(painter, start, finish, &edge, &resolved, colours);
        };
        if connection_bar(ui, &from, &to, &mut used, colours, paint) {
            rule = if used { EdgeRule::default() } else { EdgeRule::NOTHING };
        }
        if !used {
            set_edge_rule(&mut self.edge_rules, edge, rule);
            return;
        }
        // The end that takes just the one edge: a data stream's input, or the reading end of a value.
        let single = if edge.to.role == crate::engine::PortRole::DataStreamIn { &to } else { &from };
        let port = format!("{}'s {}", single.0, single.2);
        let others_kept = format!("Any other connections out of {}'s {} are left as they are.", from.0, from.2);
        scrolled(ui, |ui| {
            if_else(ui, colours, |ui| {
                section(ui, "If they're connected already", colours, |ui| {
                    section(ui, "Enable or disable", colours, |ui| {
                        for (choice, text) in [(None, DO_NOTHING), (Some(true), "Enable"), (Some(false), "Disable")] {
                            radio(ui, &mut rule.enabled, choice, text);
                        }
                    });
                });
                section_with_info(ui, &format!("Else, if {port} is free"), Some(&others_kept), colours, |ui| {
                    radio(ui, &mut rule.connect, false, DO_NOTHING);
                    radio(ui, &mut rule.connect, true, "Connect them");
                });
                let title = format!("Else, if {port} is occupied");
                section_with_info(ui, &title, Some(&others_kept), colours, |ui| {
                    radio(ui, &mut rule.replace, false, DO_NOTHING);
                    radio(ui, &mut rule.replace, true, "Connect them, replacing that");
                });
            });
        });
        set_edge_rule(&mut self.edge_rules, edge, rule);
    }

    fn node_inspector(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        graph: &Graph<N>,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
        colours: &Colours,
    ) {
        let others: Vec<(NodeId, String)> =
            self.graph.iter().filter(|(o, _)| *o != id).map(|(o, e)| (o, e.label.clone())).collect();
        let Some(entry) = self.graph.get_mut(id) else { return };
        let rule = self.rules.entry(id).or_default();
        let info = entry.node.info();
        let mut used = rule.mode != Mode::Ignore;
        let colour = style::category_colour(info.category);
        if title_bar(ui, &entry.label, Some(colour), Some(&mut used), colours) {
            rule.mode = if used { Mode::Ensure } else { Mode::Ignore };
        }
        if !used {
            return;
        }
        scrolled(ui, |ui| {
            let found_as = found.as_ref().ok().and_then(|f| f.get(&id).copied()).and_then(|other| graph.get(other));
            let disambiguation = "Nodes with only one possible match are paired first; the rest pair up left to right.";
            section_with_info(ui, "Find a node that matches", Some(disambiguation), colours, |ui| {
                leaf(ui.label(format!("Type: {}", info.name)).rect);
                let mut pattern = rule.label_pattern.clone().unwrap_or_else(|| ANY_LABEL.to_string());
                let row = ui.horizontal(|ui| {
                    let label = ui.label("Label pattern");
                    // Room after the box for the ⓘ.
                    let width = (ui.available_width() - 24.0).max(60.0);
                    let edit = egui::TextEdit::singleline(&mut pattern).desired_width(width);
                    let edited = ui.add(edit).labelled_by(label.id);
                    info_icon_with(ui, regex_cheat_sheet);
                    edited.changed()
                });
                leaf(row.response.rect);
                if row.inner {
                    rule.label_pattern = (pattern != ANY_LABEL).then(|| pattern.clone());
                }
                let problem = match regex::Regex::new(&pattern) {
                    _ if pattern.is_empty() => Some("Empty, it matches nothing: .* matches any label".to_string()),
                    Err(e) => {
                        Some(format!("Not a valid pattern: {}", e.to_string().lines().last().unwrap_or("").trim()))
                    }
                    Ok(_) => None,
                };
                if let Some(problem) = problem {
                    leaf(ui.label(RichText::new(problem).small().color(colours.error)).rect);
                }
            });

            if_else(ui, colours, |ui| {
                section(ui, "If a matching node is found", colours, |ui| {
                    section(ui, "Enable or disable", colours, |ui| {
                        for (choice, text) in [(None, DO_NOTHING), (Some(true), "Enable"), (Some(false), "Disable")] {
                            radio(ui, &mut rule.enabled, choice, text);
                        }
                    });
                    let settings = serde_json::to_value(&entry.node).ok().and_then(|v| v.get("settings").cloned());
                    let keys: Vec<String> = (settings.as_ref().and_then(|s| s.as_object()))
                        .map(|o| o.keys().cloned().collect())
                        .unwrap_or_default();
                    if keys.is_empty() {
                        return;
                    }
                    let applying = &mut self.applying;
                    section(ui, "Settings", colours, |ui| {
                        let mut apply = applying.contains(&id) || !rule.settings.is_empty();
                        radio(ui, &mut apply, false, DO_NOTHING);
                        let apply_row = radio(ui, &mut apply, true, "Apply settings");
                        if !apply {
                            applying.remove(&id);
                            rule.settings.clear();
                            return;
                        }
                        applying.insert(id);
                        nested(ui, apply_row, colours, |ui| {
                            for key in &keys {
                                let mut on = rule.settings.contains(key);
                                let checkbox = ui.checkbox(&mut on, key.replace('_', " "));
                                leaf(checkbox.rect);
                                if checkbox.changed() {
                                    match on {
                                        true => rule.settings.push(key.clone()),
                                        false => rule.settings.retain(|k| k != key),
                                    }
                                }
                            }
                            if let Some(other) = found_as {
                                let take = ui.horizontal(|ui| {
                                    if ui.button(format!("Take the settings of {}", other.label)).clicked() {
                                        entry.node = other.node.clone();
                                    }
                                    let info = format!(
                                        "In the graph now, {} is the node found for this one. Its current settings \
                                     replace the settings here.",
                                        other.label
                                    );
                                    info_icon(ui, &info);
                                });
                                leaf(take.response.rect);
                            }
                        });
                    });
                });

                section(ui, "Else, if no matching node is found", colours, |ui| {
                    for mode in [Mode::IfPresent, Mode::Ensure, Mode::Require] {
                        radio(ui, &mut rule.mode, mode, mode.choice());
                    }
                });
            });

            window_ui(ui, &mut rule.window, &others, colours);
        });
    }

    fn action_inspector(
        &mut self,
        ui: &mut egui::Ui,
        layout: &Layout,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
        base_directories: &[&'static str],
        colours: &Colours,
    ) {
        title_bar(ui, "Quick action", None, None, colours);
        scrolled(ui, |ui| {
            let action = &mut self.action;
            let name = ui.label("Name");
            ui.text_edit_singleline(&mut action.name).labelled_by(name.id);
            let description = ui.label("Description");
            ui.add(egui::TextEdit::multiline(&mut action.description).desired_rows(3)).labelled_by(description.id);
            ui.horizontal(|ui| {
                if ui.button("Arrange").clicked() {
                    for (id, pos) in super::arrange::arrange(&self.graph) {
                        if let Some(entry) = self.graph.get_mut(id) {
                            entry.pos = pos;
                        }
                    }
                    self.canvas.frame_all();
                }
            });

            section(ui, "Panes", colours, |ui| {
                let action = &mut self.action;
                let mut arrange = action.panes.arrangement.is_some();
                let row = ui.horizontal(|ui| {
                let ticked = ui.checkbox(&mut arrange, "Arrange as they are now");
                info_icon(
                    ui,
                    "When the quick action runs, home and the windows of the nodes found are laid out as the app has \
                     them now.",
                );
                ticked
            });
                leaf(row.response.rect);
                let ticked = row.inner;
                let again = arrange && {
                    let again = ui.small_button("As they are now, again");
                    leaf(again.rect);
                    again.clicked()
                };
                if ticked.changed() && !arrange {
                    action.panes.arrangement = None;
                } else if ticked.changed() || again {
                    let ids: Vec<(NodeId, NodeId)> =
                        (found.iter()).flat_map(|f| f.iter()).map(|(role, id)| (*id, *role)).collect();
                    action.panes.arrangement = layout
                        .arrangement()
                        .and_then(|a| a.map_windows(&|id| ids.iter().find(|(node, _)| *node == id).map(|p| p.1)));
                }
            });

            section(ui, "Base directory", colours, |ui| {
                let action = &mut self.action;
                let label = ui.label("Choose");
                leaf(label.rect);
                let current = action.base_directory.clone().unwrap_or_else(|| DO_NOTHING.into());
                let combo = egui::ComboBox::from_id_salt("base directory")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut action.base_directory, None, DO_NOTHING);
                        for name in base_directories {
                            ui.selectable_value(&mut action.base_directory, Some(name.to_string()), *name);
                        }
                    })
                    .response
                    .labelled_by(label.id);
                leaf(combo.rect);
            });

            section(ui, "If the graph was empty", colours, |ui| {
                leaf(ui.checkbox(&mut self.action.arrange_if_empty, "Lay out the graph afresh").rect);
            });
        });
    }
}

/// A node's window, if open, is left as it is or closed; if not, left as it is or opened, going (the first time)
/// where `Placement` says, relative to the window of one of `others` (the quick action's other nodes, with their
/// labels) for some.
fn window_ui(ui: &mut egui::Ui, window: &mut Window, others: &[(NodeId, String)], colours: &Colours) {
    if_else(ui, colours, |ui| {
        section(ui, "If the node's window is open", colours, |ui| {
            radio(ui, &mut window.close, false, DO_NOTHING);
            radio(ui, &mut window.close, true, "Close it");
        });
        section(ui, "Else, if the node's window isn't open", colours, |ui| {
            let opening = window.open.is_some();
            radio(ui, &mut window.open, None, DO_NOTHING);
            let open = ui.radio(opening, "Open it");
            leaf(open.rect);
            if open.clicked() && !opening {
                window.open = Some(Placement::Default);
            }
            let Some(placement) = &mut window.open else { return };
            let info = "A window that's been open before opens where it was. Placed by another window that isn't \
                        open, a window goes where a new window goes.";
            section_with_info(ui, "Where, the first time", Some(info), colours, |ui| {
                placement_ui(ui, placement, others)
            });
        });
    });
}

fn placement_ui(ui: &mut egui::Ui, placement: &mut Placement, others: &[(NodeId, String)]) {
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
    for (i, (name, choice)) in kinds.iter().enumerate() {
        let Some(choice) = choice else { continue };
        let button = ui.radio(i == current, *name);
        leaf(button.rect);
        if button.clicked() && i != current {
            *placement = *choice;
        }
    }

    let name = |of: NodeId| others.iter().find(|(o, _)| *o == of).map_or("?", |(_, l)| l.as_str()).to_string();
    let which = |ui: &mut egui::Ui, of: &mut NodeId| {
        let label = ui.label("Whose window");
        leaf(label.rect);
        let combo = egui::ComboBox::from_id_salt("anchor")
            .selected_text(name(*of))
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for (other, label) in others {
                    ui.selectable_value(of, *other, label);
                }
            })
            .response
            .labelled_by(label.id);
        leaf(combo.rect);
    };
    match placement {
        Placement::Behind(of) => which(ui, of),
        Placement::Beside { of, side, share } => {
            which(ui, of);
            let row = ui.horizontal(|ui| {
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
            leaf(row.response.rect);
        }
        Placement::Default | Placement::Largest => {}
    }
}

/// The editor says "do nothing" the same way everywhere, as the first option: to the node found, when none is
/// found, to a window, to a connection, and to the base directory.
const DO_NOTHING: &str = "Do nothing";

/// A connection's title, in a box with a checkbox at its top right (ticked while the quick action uses the connection;
/// unticked, it does nothing to it): its two ends, one above the other, each drawn as a node is (a bar in the node's
/// colour, with its label), with its port on its right edge, named beside it; and between the ports, outside the
/// nodes, the connection (drawn by `paint`, given the ports' places), as the canvas draws it. This returns whether the
/// checkbox was clicked.
fn connection_bar(
    ui: &mut egui::Ui,
    from: &(String, egui::Color32, String),
    to: &(String, egui::Color32, String),
    used: &mut bool,
    colours: &Colours,
    paint: impl FnOnce(&egui::Painter, egui::Pos2, egui::Pos2),
) -> bool {
    // Room at the right of the nodes for the wire, looping out from one port to the other.
    const LOOP: f32 = 40.0;
    let mut clicked = false;
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, colours.border))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Connection").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let checked = *used;
                    let response = ui.add(egui::Checkbox::without_text(used));
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::CheckBox, true, checked, "Use this connection")
                    });
                    clicked = response.changed();
                });
            });
            ui.add_space(4.0);
            let width = ui.available_width() - LOOP;
            let node = |ui: &mut egui::Ui, (label, colour, port): &(String, egui::Color32, String)| {
                let frame = egui::Frame::new()
                    .fill(colours.surface_raised)
                    .stroke(egui::Stroke::new(1.0, colours.border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin { left: 8, right: 12, top: 5, bottom: 5 });
                let shown = frame.show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    ui.horizontal(|ui| {
                        ui.label(super::docking::tab_title(label, *colour));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(port).color(colours.text_primary));
                        });
                    });
                });
                let rect = shown.response.rect;
                egui::pos2(rect.right(), rect.center().y)
            };
            let from = node(ui, from);
            ui.add_space(8.0);
            let to = node(ui, to);
            paint(ui.painter(), from, to);
        });
    ui.add_space(4.0);
    clicked
}

/// Under its bar (see `title_bar`), what the inspector shows scrolls, if it's taller than the room left.
fn scrolled(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false, false]).show(ui, add);
}

/// The inspector shows what it's showing in a bar at its top, as a pane's tab does: a node's colour and label, say.
/// A node's bar has a checkbox at its right, ticked while the quick action uses the node (otherwise it's ignored: see
/// `Mode::Ignore`); this returns whether it was clicked.
fn title_bar(
    ui: &mut egui::Ui,
    title: &str,
    colour: Option<egui::Color32>,
    used: Option<&mut bool>,
    colours: &Colours,
) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(colours.surface_raised)
        .stroke(egui::Stroke::new(1.0, colours.border))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match colour {
                    Some(colour) => ui.label(super::docking::tab_title(title, colour)),
                    None => ui.label(RichText::new(title).size(14.0)),
                };
                if let Some(used) = used {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let checked = *used;
                        let accessible = format!("Use {title}");
                        let response = ui.add(egui::Checkbox::without_text(used));
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(egui::Role::CheckBox, true, checked, accessible.clone())
                        });
                        clicked = response.changed();
                    });
                }
            });
        });
    ui.add_space(4.0);
    clicked
}

// The inspector's sections are drawn as a tree: each section's line runs from its title down to its last row, with a
// stub out to each row, and a dot where a section within it branches off. Rows say where they are (`leaf`) to the
// section they're in, kept on this stack while it's drawn.
thread_local! {
    static TREE: std::cell::RefCell<Vec<Vec<(egui::Rect, bool)>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A row of a section (an option, say) says where it is with this, for its stub from the section's line.
fn leaf(rect: egui::Rect) {
    branch_at(rect, false);
}

fn branch_at(rect: egui::Rect, heading: bool) {
    TREE.with(|tree| {
        if let Some(rows) = tree.borrow_mut().last_mut() {
            rows.push((rect, heading));
        }
    });
}

/// What `add` adds, indented, and the rows (and sections' titles, marked) it says are in it.
fn rows<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> (egui::Rect, Vec<(egui::Rect, bool)>, R) {
    TREE.with(|tree| tree.borrow_mut().push(Vec::new()));
    let inner = ui.horizontal_top(|ui| {
        ui.add_space(14.0);
        ui.vertical(add).inner
    });
    let rows = TREE.with(|tree| tree.borrow_mut().pop()).unwrap_or_default();
    (inner.response.rect, rows, inner.inner)
}

/// A section's line, at `x`, from `top` to its last row (or to the bottom of what's in it, if something after its
/// last row is), a stub to each row, and a dot where a section branches off.
fn paint_tree(ui: &egui::Ui, x: f32, top: f32, area: egui::Rect, rows: &[(egui::Rect, bool)], colours: &Colours) {
    let colour = colours.text_secondary.gamma_multiply(0.5);
    let stroke = egui::Stroke::new(1.5, colour);
    let painter = ui.painter();
    let bottom = match rows.last() {
        Some((rect, heading)) if *heading || area.bottom() - rect.bottom() < 6.0 => rect.center().y,
        _ => area.bottom() - 4.0,
    };
    painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
    for (rect, heading) in rows {
        let y = rect.center().y;
        painter.line_segment([egui::pos2(x, y), egui::pos2(rect.left() - 3.0, y)], stroke);
        if *heading {
            painter.circle_filled(egui::pos2(x, y), 3.0, colour);
        }
    }
}

/// A section of the inspector: its title, in bold, and under it what's in it, indented, in the tree (see `TREE`).
fn section<R>(ui: &mut egui::Ui, title: &str, colours: &Colours, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    section_with_info(ui, title, None, colours, add)
}

/// A section (see `section`) with an ⓘ beside its title, for what isn't plain from what's in it.
fn section_with_info<R>(
    ui: &mut egui::Ui,
    title: &str,
    info: Option<&str>,
    colours: &Colours,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.add_space(4.0);
    let heading = match info {
        Some(info) => {
            ui.horizontal(|ui| {
                let heading = bold(ui, title, colours.text_primary);
                info_icon(ui, info);
                heading
            })
            .inner
        }
        None => bold(ui, title, colours.text_primary),
    };
    branch_at(heading, true);
    let (area, rows, inner) = rows(ui, add);
    paint_tree(ui, heading.left() + 4.5, heading.bottom() + 1.0, area, &rows, colours);
    inner
}

/// What follows from an option (the settings to apply, once applying them's chosen, say), indented under it, its
/// rows joined to a line down from the option's button.
fn nested<R>(ui: &mut egui::Ui, option: egui::Rect, colours: &Colours, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (area, rows, inner) = rows(ui, add);
    let x = option.left() + ui.spacing().icon_width / 2.0;
    paint_tree(ui, x, option.bottom(), area, &rows, colours);
    inner
}

/// Sections that are one choice (if this, do one thing; else, if that, another) branch off one line down the left,
/// from the first to the last.
fn if_else(ui: &mut egui::Ui, colours: &Colours, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    let (area, rows, ()) = rows(ui, add);
    let top = rows.first().map_or(area.top(), |(rect, _)| rect.center().y);
    paint_tree(ui, area.left() + 4.5, top, area, &rows, colours);
}

/// The app's fonts have no bold, so a title's drawn twice, a fraction of a pixel apart, to look it.
fn bold(ui: &mut egui::Ui, text: &str, colour: egui::Color32) -> egui::Rect {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, colour);
    let (rect, response) = ui.allocate_exact_size(galley.size() + egui::vec2(0.6, 0.0), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    ui.painter().galley(rect.min, galley.clone(), colour);
    ui.painter().galley(rect.min + egui::vec2(0.6, 0.0), galley, colour);
    rect
}

/// A choice in the inspector, as a row of its section (see `leaf`).
fn radio<T: PartialEq>(ui: &mut egui::Ui, current: &mut T, choice: T, text: &str) -> egui::Rect {
    let rect = ui.radio_value(current, choice, text).rect;
    leaf(rect);
    rect
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
