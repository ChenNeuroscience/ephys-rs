//! People manage quick actions (see `crate::quick_action`) in Settings › Quick actions. Each is a file
//! (`name.quick-action.json`) in a folder beside the app's settings; a build has none of its own. Quick actions can be
//! made from the graph (the nodes selected, or all of them), changed, imported from files, copied to the clipboard (to
//! share them), and deleted. Any can be left out of the basic view.
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
use crate::base_directory::BaseDirectories;
use crate::config_patching::PatchSets;
use crate::engine::{Edge, Graph, NodeId, NodesEnum};
use crate::layout::Layout;
use crate::quick_action::{
    ANY_LABEL, Anchor, ApplyBaseDirectory, ByShape, EdgeRule, EnableOrDisable, IfFree, IfNotFound, IfOccupied,
    IfWindowClosed, IfWindowOpen, Placement, Position, QuickAction, Rule, Settings, Shape,
};
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

/// The quick actions are files in a folder: people's own, made in the editor, pasted in, or copied there (the
/// release's zip has some to start with).
pub struct QuickActions<N> {
    /// The quick actions' files are in this folder, if there is one.
    pub folder: Option<PathBuf>,
    /// Without a folder (in tests), the quick actions are read from these texts instead, with no files.
    texts: &'static [&'static str],
    pub entries: Vec<Entry<N>>,
    /// What couldn't be read, and why, a line for each.
    pub problems: Vec<String>,
}

impl<N> Default for QuickActions<N> {
    fn default() -> Self {
        QuickActions { folder: None, texts: &[], entries: Vec::new(), problems: Vec::new() }
    }
}

impl<N: NodesEnum> QuickActions<N> {
    /// Quick actions read from texts, with no folder: tests' (see `crate::gui::App::with_quick_actions`).
    #[cfg(test)]
    pub fn from_texts(texts: &'static [&'static str], patches: &PatchSets) -> Self {
        let mut actions = QuickActions { texts, ..Default::default() };
        actions.reload(patches);
        actions
    }

    /// Reads the quick actions again, from the folder, in order of their files' names; or, without a folder, from
    /// the texts (see `from_texts`).
    pub fn reload(&mut self, patches: &PatchSets) {
        self.entries.clear();
        self.problems.clear();
        let Some(folder) = &self.folder else {
            for text in self.texts {
                match QuickAction::parse(text, patches) {
                    Ok(action) => self.entries.push(Entry { action, file: None, modified: None }),
                    Err(problem) => self.problems.push(problem),
                }
            }
            return;
        };
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

    /// Copies the quick actions in `files` into the folder, each as it is, to a new file named for it; returns how many
    /// were copied, and what couldn't be (a file that isn't a quick action, or can't be read here), and why.
    pub fn import(&self, files: &[PathBuf], patches: &PatchSets) -> (usize, Vec<String>) {
        let Some(folder) = &self.folder else { return (0, vec!["There's no folder for quick actions".into()]) };
        let (mut imported, mut problems) = (0, Vec::new());
        for file in files {
            let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let text = match std::fs::read_to_string(file) {
                Ok(text) => text,
                Err(e) => {
                    problems.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let action = match QuickAction::<N>::parse(&text, patches) {
                Ok(action) => action,
                Err(problem) => {
                    problems.push(format!("{name}: {problem}"));
                    continue;
                }
            };
            let copied = std::fs::create_dir_all(folder).and_then(|()| {
                let to = new_file(folder, &action.name);
                std::fs::write(to, &text)
            });
            match copied {
                Ok(()) => imported += 1,
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
        (imported, problems)
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
    Delete(usize),
    /// Files chosen, to copy into the folder.
    Import,
    OpenFolder,
}

/// Settings › Quick actions: the folder they're in (with a button to open it), buttons to make a new one and to import
/// files, what couldn't be read, and the list of them, scrolling, each offered in the basic view or not (`hidden`, by
/// name), with buttons to edit and delete it. What's asked for is added to `requests`.
pub fn manager_ui<N: NodesEnum>(
    ui: &mut egui::Ui,
    actions: &QuickActions<N>,
    hidden: &mut BTreeSet<String>,
    selected: usize,
    colours: &Colours,
    requests: &mut Vec<Request>,
) {
    let weak = |text: &str| RichText::new(text).small().color(colours.text_secondary);
    // Where the files are, with a button to open the folder, on a strip of its own.
    if let Some(folder) = &actions.folder {
        egui::Frame::new()
            .fill(colours.surface_raised)
            .stroke(egui::Stroke::new(1.0, colours.border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let open = toolbar::small_icon_button(ui, Icon::Open, "Open the folder", "Open the folder");
                    if open.clicked() {
                        requests.push(Request::OpenFolder);
                    }
                    let path = RichText::new(folder.display().to_string()).color(colours.text_primary);
                    ui.add(egui::Label::new(path).truncate()).on_hover_text(folder.display().to_string());
                });
            });
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        let new = match selected {
            0 => "New from the graph".to_string(),
            1 => "New from the selected node".to_string(),
            n => format!("New from the {n} selected nodes"),
        };
        if ui.button(new).clicked() {
            requests.push(Request::New);
        }
        if actions.folder.is_some() && ui.button("Import…").on_hover_text("Quick actions' files").clicked() {
            requests.push(Request::Import);
        }
    });
    for problem in &actions.problems {
        ui.label(RichText::new(problem).small().color(colours.warning));
    }
    ui.add_space(6.0);
    // The list fills the width, and scrolls in what's left of the height, its rows striped.
    egui::ScrollArea::vertical().id_salt("quick action list").auto_shrink([false, false]).show(ui, |ui| {
        for (index, entry) in actions.entries.iter().enumerate() {
            let fill = if index % 2 == 1 { colours.surface_raised } else { egui::Color32::TRANSPARENT };
            egui::Frame::new().fill(fill).corner_radius(4.0).inner_margin(egui::Margin::symmetric(6, 5)).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let name = &entry.action.name;
                        let mut on = !hidden.contains(name);
                        let offered =
                            ui.add(egui::Checkbox::without_text(&mut on)).on_hover_text("Offered in the basic view");
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
                        // Room at the right for the buttons.
                        let text_width = (ui.available_width() - 56.0).max(120.0);
                        ui.vertical(|ui| {
                            ui.set_width(text_width);
                            ui.strong(name);
                            if !entry.action.description.is_empty() {
                                ui.label(weak(&entry.action.description));
                            }
                            if let Some(modified) = entry.modified {
                                let local: chrono::DateTime<chrono::Local> = modified.into();
                                ui.label(weak(&format!("Updated {}", local.format("%-d %b %Y, %H:%M"))));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if entry.file.is_some()
                                && toolbar::small_icon_button(ui, Icon::Delete, "Delete", "Delete the file").clicked()
                            {
                                requests.push(Request::Delete(index));
                            }
                            if toolbar::small_icon_button(ui, Icon::Edit, "Edit", "Edit").clicked() {
                                requests.push(Request::Edit(index));
                            }
                        });
                    });
                },
            );
        }
    });
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
    /// The inspector's "Quick action" tab's in front, with something selected (whose tab's behind it).
    on_quick_action_tab: bool,
    /// What was selected last frame, so something newly selected brings its tab to the front.
    last_selected: (BTreeSet<NodeId>, Option<Edge>),
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
    /// How many settings each node has, for saying how many of them are applied.
    setting_counts: BTreeMap<NodeId, usize>,
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
        let colour = match rule.if_not_found {
            IfNotFound::DontOffer => self.colours.accent,
            IfNotFound::Create => self.colours.success,
            IfNotFound::DoNothing => self.colours.text_secondary,
        };
        if !rule.used {
            return None;
        }
        // In the inspector's words, leaving out what does nothing (but for what happens if it isn't found).
        let mut lines = Vec::new();
        let mut if_found = Vec::new();
        match rule.if_found.enable_or_disable {
            EnableOrDisable::Enable => if_found.push("enable".to_string()),
            EnableOrDisable::Disable => if_found.push("disable".to_string()),
            EnableOrDisable::DoNothing => {}
        }
        if let Settings::Apply(names) = &rule.if_found.settings
            && !names.is_empty()
        {
            let of = self.setting_counts.get(&id).copied().unwrap_or(names.len());
            if_found.push(format!("apply {}/{of} settings", names.len()));
        }
        if !if_found.is_empty() {
            lines.push(format!("If found: {}", if_found.join(", ")));
        }
        let otherwise = match rule.if_not_found {
            IfNotFound::Create => "create the node",
            IfNotFound::DontOffer => "don't offer the quick action",
            IfNotFound::DoNothing => "do nothing",
        };
        // "Else" follows an "If found"; on its own, it says its case.
        let case = if lines.is_empty() { "If not found" } else { "Else" };
        lines.push(format!("{case}: {otherwise}"));
        let mut window = Vec::new();
        if rule.if_window_open == IfWindowOpen::Close {
            window.push("close it if open");
        }
        if rule.if_window_closed != IfWindowClosed::DoNothing {
            window.push("open it if not");
        }
        if !window.is_empty() {
            lines.push(format!("Window: {}", window.join(", ")));
        }
        Some((lines.join("\n"), colour))
    }

    fn edge_colour(&self, edge: &Edge) -> Option<egui::Color32> {
        let rule = edge_rule(self.edge_rules, *edge);
        if rule.if_occupied == IfOccupied::Swap { Some(self.colours.warning) } else { None }
    }

    fn node_faded(&self, id: NodeId) -> bool {
        self.ignored(id)
    }

    fn edge_faded(&self, edge: &Edge) -> bool {
        edge_rule(self.edge_rules, *edge).does_nothing() || self.ignored(edge.from.node) || self.ignored(edge.to.node)
    }

    fn node_menu(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let rule = self.rules.entry(id).or_default();
        ui.checkbox(&mut rule.used, "Use this node");
        if rule.used {
            ui.separator();
            ui.weak("If not found");
            for choice in IfNotFound::ALL {
                ui.radio_value(&mut rule.if_not_found, choice, choice.choice());
            }
        }
    }

    fn edge_menu(&mut self, ui: &mut egui::Ui, edge: Edge) {
        let mut rule = edge_rule(self.edge_rules, edge);
        // A connection with a node at either end ignored is ignored too (see `Selected::locked`).
        let ends = !self.ignored(edge.from.node) && !self.ignored(edge.to.node);
        let mut used = ends && !rule.does_nothing();
        let checkbox = ui.add_enabled(ends, egui::Checkbox::new(&mut used, "Use this connection"));
        if checkbox.on_disabled_hover_text(ENDS_IGNORED).changed() {
            rule = if used { EdgeRule::default() } else { EdgeRule::NOTHING };
        }
        if used {
            ui.separator();
            ui.radio_value(&mut rule.if_free, IfFree::DoNothing, "If free: do nothing");
            ui.radio_value(&mut rule.if_free, IfFree::Connect, "If free: connect");
            ui.separator();
            ui.radio_value(&mut rule.if_occupied, IfOccupied::DoNothing, "If occupied: do nothing");
            ui.radio_value(&mut rule.if_occupied, IfOccupied::Swap, "If occupied: swap");
        }
        set_edge_rule(self.edge_rules, edge, rule);
    }
}

impl Marks<'_> {
    fn ignored(&self, id: NodeId) -> bool {
        self.rules.get(&id).is_some_and(|r| !r.used)
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

/// A connection with a node at either end ignored is ignored too; its power button says why.
const ENDS_IGNORED: &str = "Ignored, as a node at one end is: use both nodes to use the connection";

/// The inspector, beside the canvas, is this wide.
const INSPECTOR: f32 = 330.0;

impl<N: NodesEnum> Editor<N> {
    pub fn new(action: QuickAction<N>, file: Option<PathBuf>, colours: &'static Colours) -> Self {
        let (graph, edge_rules) = action.graph();
        let rules = action.roles.iter().map(|r| (r.id, r.rule.clone())).collect();
        let mut canvas = Canvas::new(colours);
        canvas.legend_open = false;
        canvas.id_salt = Some(egui::Id::unique("quick action canvas"));
        Editor {
            action,
            file,
            graph,
            rules,
            edge_rules,
            canvas,
            selection: Selection::default(),
            on_quick_action_tab: false,
            last_selected: Default::default(),
        }
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

    /// Tests rename a node of the quick action with this.
    #[cfg(test)]
    pub fn relabel(&mut self, id: NodeId, label: &str) {
        if let Some(entry) = self.graph.get_mut(id) {
            entry.label = label.to_string();
        }
    }

    /// Tests select a node of the quick action with this.
    #[cfg(test)]
    pub fn select(&mut self, id: NodeId) {
        self.selection.only(id);
    }

    /// Tests select several nodes of the quick action with this.
    #[cfg(test)]
    pub fn select_all_of(&mut self, ids: &[NodeId]) {
        self.selection.clear();
        self.selection.nodes.extend(ids);
    }

    /// Tests check whether the quick action uses a node with this.
    #[cfg(test)]
    pub fn uses(&self, id: NodeId) -> bool {
        self.rules.get(&id).is_none_or(|r| r.used)
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
        base_directories: &BaseDirectories,
        colours: &'static Colours,
    ) -> Outcome {
        let mut outcome = Outcome::Open;
        let found = self.action().find(graph);
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).max(700.0);
        // Room for the title bar and the footer, which are about this tall together.
        let height = (screen.height() * 0.92 - 162.0).max(300.0);
        let named = !self.action.name.trim().is_empty();
        let body = |ui: &mut egui::Ui| {
            let size = egui::vec2(ui.available_width(), height);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
                ui.set_height(height);
                // The canvas, a divider and the inspector fill the width exactly (any wider, and the dialog grows
                // past its title bar and footer), edge to edge, so nothing's spaced between them.
                let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 0.0);
                let canvas_size = egui::vec2(size.x - INSPECTOR - 1.0, height);
                ui.allocate_ui(canvas_size, |ui| {
                    ui.set_min_size(canvas_size);
                    ui.set_max_size(canvas_size);
                    self.canvas_ui::<A>(ui, colours);
                });
                // The divider, as the dialog has above and below its body.
                let (divider, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
                ui.painter().vline(divider.center().x, divider.y_range(), (1.0, colours.divider));
                let inner = egui::vec2(INSPECTOR, height);
                egui::Frame::new().fill(ui.visuals().panel_fill).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = spacing;
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    ui.vertical(|ui| self.inspector(ui, graph, layout, &found, base_directories, colours));
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
        let closed = dialog::show_edge_to_edge(ctx, "Quick action", width, colours, body, footer);
        if closed && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }

    fn canvas_ui<A: AnnotationsEnum>(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let resolved = self.graph.resolve();
        let mut annotations = Annotations::<A>::default();
        let mut events = Vec::new();
        let setting_counts = (self.graph.iter())
            .map(|(id, entry)| {
                let settings = serde_json::to_value(&entry.node).ok().and_then(|v| v.get("settings").cloned());
                (id, settings.as_ref().and_then(|s| s.as_object()).map_or(0, |o| o.len()))
            })
            .collect();
        let mut marks = Marks { rules: &mut self.rules, edge_rules: &mut self.edge_rules, setting_counts, colours };
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
        base_directories: &BaseDirectories,
        colours: &Colours,
    ) {
        // Something newly selected brings its tab to the front.
        let selected = (self.selection.nodes.clone(), self.selection.edge);
        if selected != self.last_selected {
            self.last_selected = selected;
            self.on_quick_action_tab = false;
        }
        let what = self.selected();
        let showing = what.is_some() && !self.on_quick_action_tab;
        let mut tabs = vec![(egui::WidgetText::from("Quick action"), false)];
        if let Some(what) = &what {
            tabs.push((what.title.clone(), true));
        }
        let tab = tab_bar(ui, &tabs, usize::from(showing));
        match tab {
            Some(TabEvent::Clicked(index)) => self.on_quick_action_tab = index == 0,
            Some(TabEvent::Closed) => {
                self.selection.clear();
                return;
            }
            None => {}
        }
        if let Some(what) = what.as_ref().filter(|_| !self.on_quick_action_tab) {
            // The power button, at the top of the pane, as a node's window has its buttons.
            let (mut used, mixed, locked) = (what.used, what.mixed, what.locked);
            let label = format!("Use {}", what.title.text().trim());
            let clicked = egui::Frame::new()
                .inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 0 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let layout = egui::Layout::right_to_left(egui::Align::Center);
                        ui.with_layout(layout, |ui| {
                            if locked {
                                let shown = ui.add_enabled_ui(false, |ui| use_button(ui, &mut used, mixed, &label));
                                shown.response.on_disabled_hover_text(ENDS_IGNORED);
                                false
                            } else {
                                use_button(ui, &mut used, mixed, &label)
                            }
                        })
                        .inner
                    })
                    .inner
                })
                .inner;
            if clicked {
                self.set_used(used);
            }
        }
        if !self.on_quick_action_tab {
            if let Some(edge) = self.selection.edge {
                return self.edge_inspector(ui, edge, colours);
            }
            if let Some(id) = self.selection.single() {
                return self.node_inspector(ui, id, graph, found, colours);
            }
            if self.selection.nodes.len() > 1 {
                return self.nodes_inspector(ui, colours);
            }
        }
        self.action_inspector(ui, layout, found, base_directories, colours);
    }

    /// Whether a node at either end of `edge` is ignored, so that the connection is too (see `Selected::locked`).
    fn ignored_end(&self, edge: Edge) -> bool {
        [edge.from.node, edge.to.node].iter().any(|id| self.rules.get(id).is_some_and(|r| !r.used))
    }

    /// What's selected, for its tab: its title, and whether the quick action uses it (or, for several nodes, some
    /// of them: mixed).
    fn selected(&self) -> Option<Selected> {
        let used = |id: &NodeId| self.rules.get(id).is_none_or(|r| r.used);
        if let Some(edge) = self.selection.edge {
            // A connection's matched only with both its ends: with either ignored, so is the connection, whatever its
            // own rule.
            let ends = used(&edge.from.node) && used(&edge.to.node);
            let used = ends && !edge_rule(&self.edge_rules, edge).does_nothing();
            return Some(Selected { title: "Connection".into(), used, mixed: false, locked: !ends });
        }
        if let Some(id) = self.selection.single() {
            let entry = self.graph.get(id)?;
            let title = super::docking::tab_title(&entry.label, style::category_colour(entry.node.info().category));
            return Some(Selected { title, used: used(&id), mixed: false, locked: false });
        }
        let count = self.selection.nodes.len();
        (count > 1).then(|| {
            let some = self.selection.nodes.iter().filter(|id| used(id)).count();
            let mixed = some > 0 && some < count;
            Selected { title: format!("{count} nodes").into(), used: some == count, mixed, locked: false }
        })
    }

    /// The power button in the selection's tab uses what's selected, or ignores it: a node (all of several), or a
    /// connection.
    fn set_used(&mut self, used: bool) {
        if let Some(edge) = self.selection.edge {
            set_edge_rule(&mut self.edge_rules, edge, if used { EdgeRule::default() } else { EdgeRule::NOTHING });
            return;
        }
        for id in &self.selection.nodes {
            self.rules.entry(*id).or_default().used = used;
        }
    }

    /// Several nodes selected: a box of them, left to right, each as its bar, with a checkbox for all of them at its top
    /// right, ticked while they're all used, empty while none is, and marked as mixed otherwise (clicked, it uses
    /// them all).
    fn nodes_inspector(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let mut nodes: Vec<(NodeId, String, egui::Color32, [f32; 2])> = (self.selection.nodes.iter())
            .filter_map(|id| {
                let entry = self.graph.get(*id)?;
                let colour = style::category_colour(entry.node.info().category);
                Some((*id, entry.label.clone(), colour, entry.pos))
            })
            .collect();
        nodes.sort_by(|a, b| a.3[0].total_cmp(&b.3[0]).then(a.3[1].total_cmp(&b.3[1])));
        under_bar(ui, |ui| {
            for (_, label, colour, _) in &nodes {
                egui::Frame::new()
                    .fill(colours.surface_raised)
                    .stroke(egui::Stroke::new(1.0, colours.border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(super::docking::tab_title(label, *colour));
                    });
                ui.add_space(2.0);
            }
        });
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
        let paint = |painter: &egui::Painter, start, finish| {
            super::canvas::paint_edge(painter, start, finish, &edge, &resolved, colours);
        };
        connection_diagram(ui, &from, &to, colours, paint);
        if rule.does_nothing() || self.ignored_end(edge) {
            return;
        }
        // The end that takes just the one edge: a data stream's input, or the reading end of a value.
        let single = if edge.to.role == crate::engine::PortRole::DataStreamIn { &to } else { &from };
        let port = format!("{}'s {}", single.0, single.2);
        let others_kept = format!("Any other connections out of {}'s {} are left as they are.", from.0, from.2);
        scrolled(ui, |ui| {
            if_else(ui, colours, |ui| {
                section(ui, "If they're connected already", colours, |ui| {
                    section(ui, "Enabled/disabled", colours, |ui| {
                        enable_or_disable(ui, &mut rule.if_connected);
                    });
                });
                section_with_info(ui, &format!("Else, if {port} is free"), Some(&others_kept), colours, |ui| {
                    radio(ui, &mut rule.if_free, IfFree::DoNothing, DO_NOTHING);
                    radio(ui, &mut rule.if_free, IfFree::Connect, &format!("Connect {}", from.0));
                });
                let title = format!("Else, if {port} is occupied");
                section_with_info(ui, &title, Some(&others_kept), colours, |ui| {
                    radio(ui, &mut rule.if_occupied, IfOccupied::DoNothing, DO_NOTHING);
                    radio(ui, &mut rule.if_occupied, IfOccupied::Swap, &format!("Swap to {}", from.0));
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
        if !rule.used {
            return;
        }
        scrolled(ui, |ui| {
            let found_as = found.as_ref().ok().and_then(|f| f.get(&id).copied()).and_then(|other| graph.get(other));
            let disambiguation = "Nodes with only one possible match are paired first; the rest pair up left to right.";
            section_with_info(ui, "Find a node that matches", Some(disambiguation), colours, |ui| {
                leaf(ui.label(format!("Type: {}", info.name)).rect);
                let mut pattern = rule.matching.label.clone().unwrap_or_else(|| ANY_LABEL.to_string());
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
                    rule.matching.label = (pattern != ANY_LABEL).then(|| pattern.clone());
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
                    section(ui, "Enabled/disabled", colours, |ui| {
                        enable_or_disable(ui, &mut rule.if_found.enable_or_disable);
                    });
                    let settings = serde_json::to_value(&entry.node).ok().and_then(|v| v.get("settings").cloned());
                    let keys: Vec<String> = (settings.as_ref().and_then(|s| s.as_object()))
                        .map(|o| o.keys().cloned().collect())
                        .unwrap_or_default();
                    if keys.is_empty() {
                        return;
                    }
                    section(ui, "Settings", colours, |ui| {
                        let settings = &mut rule.if_found.settings;
                        let mut apply = matches!(settings, Settings::Apply(_));
                        // The values applied come from the node matched in the graph now, so without one, which are
                        // applied can be kept, or dropped, but not changed.
                        let matched = found_as.is_some();
                        radio(ui, &mut apply, false, DO_NOTHING);
                        let apply_row = ui.horizontal(|ui| {
                            let can_apply = matched || apply;
                            let row =
                                ui.add_enabled_ui(can_apply, |ui| ui.radio_value(&mut apply, true, "Apply settings"));
                            info_icon(ui, APPLYING);
                            row.inner.rect
                        });
                        leaf(apply_row.response.rect);
                        let apply_row = apply_row.inner;
                        if !apply {
                            *settings = Settings::DoNothing;
                            return;
                        }
                        if *settings == Settings::DoNothing {
                            *settings = Settings::Apply(Vec::new());
                        }
                        let Settings::Apply(names) = settings else { return };
                        nested(ui, apply_row, colours, |ui| {
                            ui.add_enabled_ui(matched, |ui| {
                                for key in &keys {
                                    let mut on = names.contains(key);
                                    let checkbox = ui.checkbox(&mut on, key.replace('_', " "));
                                    leaf(checkbox.rect);
                                    if !checkbox.changed() {
                                        continue;
                                    }
                                    if on {
                                        names.push(key.clone());
                                        if let Some(other) = found_as {
                                            copy_setting(&mut entry.node, &other.node, key);
                                        }
                                    } else {
                                        names.retain(|k| k != key);
                                    }
                                }
                            });
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
                    for choice in IfNotFound::ALL {
                        radio(ui, &mut rule.if_not_found, choice, choice.choice());
                    }
                });
            });

            window_ui(ui, &mut rule.if_window_open, &mut rule.if_window_closed, &others, colours);
        });
    }

    fn action_inspector(
        &mut self,
        ui: &mut egui::Ui,
        layout: &Layout,
        found: &Result<BTreeMap<NodeId, NodeId>, String>,
        base_directories: &BaseDirectories,
        colours: &Colours,
    ) {
        scrolled(ui, |ui| {
            let action = &mut self.action;
            let name = ui.label("Name");
            // Both the pane's width.
            let width = ui.available_width();
            ui.add(egui::TextEdit::singleline(&mut action.name).desired_width(width)).labelled_by(name.id);
            let description = ui.label("Description");
            let edit = egui::TextEdit::multiline(&mut action.description).desired_rows(3).desired_width(width);
            ui.add(edit).labelled_by(description.id);

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
                // As settings are applied: the mode's the one the app has now, which can't be changed here.
                let current = &mut self.action.base_directory;
                radio(ui, current, None, DO_NOTHING);
                let mut apply = current.is_some();
                let row = ui.horizontal(|ui| {
                    let clicked = ui.radio(apply, "Apply mode").clicked();
                    info_icon(ui, APPLYING_MODE);
                    clicked
                });
                leaf(row.response.rect);
                if row.inner && !apply {
                    apply = true;
                    *current = base_directories.mode().map(|mode| ApplyBaseDirectory { mode: mode.to_string() });
                }
                if let Some(chosen) = current.as_ref().filter(|_| apply) {
                    nested(ui, row.response.rect, colours, |ui| {
                        leaf(ui.label(RichText::new(&chosen.mode).weak()).rect)
                    });
                }
            });
        });
    }
}

/// A node's window, if open, is left as it is or closed; if not, left as it is or opened, going (the first time)
/// where `Placement` says, relative to the window of one of `others` (the quick action's other nodes, with their
/// labels) for some.
fn window_ui(
    ui: &mut egui::Ui,
    if_open: &mut IfWindowOpen,
    if_closed: &mut IfWindowClosed,
    others: &[(NodeId, String)],
    colours: &Colours,
) {
    if_else(ui, colours, |ui| {
        section(ui, "If the node's window is open", colours, |ui| {
            radio(ui, if_open, IfWindowOpen::DoNothing, DO_NOTHING);
            radio(ui, if_open, IfWindowOpen::Close, "Close it");
        });
        section(ui, "Else, if the node's window isn't open", colours, |ui| {
            let opening = matches!(if_closed, IfWindowClosed::Open(_));
            radio(ui, if_closed, IfWindowClosed::DoNothing, DO_NOTHING);
            let open = ui.horizontal(|ui| {
                let open = ui.radio(opening, "Open it");
                info_icon(
                    ui,
                    "A window that's been open before reopens where it was. The choices below are for the first time \
                     it opens.",
                );
                open
            });
            leaf(open.response.rect);
            if open.inner.clicked() && !opening {
                *if_closed = IfWindowClosed::Open(Placement::Default);
            }
            let IfWindowClosed::Open(placement) = if_closed else { return };
            nested(ui, open.inner.rect, colours, |ui| placement_ui(ui, placement, others, colours));
        });
    });
}

/// Where a window opened for the first time goes: each choice, and under the one chosen, what it needs (by group shape,
/// which shape; anchored to another node's group, which node; and in either case, at the front or the back).
fn placement_ui(ui: &mut egui::Ui, placement: &mut Placement, others: &[(NodeId, String)], colours: &Colours) {
    let first_other = others.first().map(|(o, _)| *o);
    let anchored = |to| Placement::Anchored(Anchor { to, at: Position::Back });
    let kinds: Vec<(&str, Option<Placement>)> = vec![
        ("Use default location", Some(Placement::Default)),
        ("By group shape", Some(Placement::ByGroupShape(ByShape { shape: Shape::Largest, at: Position::Front }))),
        ("Anchor to group of another node", first_other.map(anchored)),
    ];
    let kind = |p: &Placement| match p {
        Placement::Default => 0,
        Placement::ByGroupShape(_) => 1,
        Placement::Anchored(_) => 2,
    };
    for (i, (text, choice)) in kinds.iter().enumerate() {
        let Some(choice) = choice else { continue };
        let current = kind(placement);
        let row = ui.horizontal(|ui| {
            let button = ui.radio(i == current, *text);
            match choice {
                Placement::Default => {
                    info_icon(ui, DEFAULT_LOCATION);
                }
                Placement::Anchored(_) => {
                    info_icon(
                        ui,
                        "If the other node's window isn't open (or the node isn't there), this window goes in the \
                         default location instead.",
                    );
                }
                Placement::ByGroupShape(_) => {}
            }
            button
        });
        leaf(row.response.rect);
        let button = row.inner;
        if button.clicked() && i != current {
            *placement = *choice;
        }
        match placement {
            Placement::ByGroupShape(by) if i == 1 => nested(ui, button.rect, colours, |ui| {
                section(ui, "Group", colours, |ui| {
                    radio(ui, &mut by.shape, Shape::Largest, "Largest");
                    radio(ui, &mut by.shape, Shape::Widest, "Widest");
                    radio(ui, &mut by.shape, Shape::Tallest, "Tallest");
                });
                section(ui, "Tab", colours, |ui| {
                    radio(ui, &mut by.at, Position::Front, "At the front");
                    radio(ui, &mut by.at, Position::Back, "At the back");
                });
            }),
            Placement::Anchored(anchor) if i == 2 => {
                nested(ui, button.rect, colours, |ui| anchored_ui(ui, anchor, others))
            }
            _ => {}
        }
    }
}

/// A window anchored to the group of another node's window: which node, and whether as a tab at the front of the
/// group or the back.
fn anchored_ui(ui: &mut egui::Ui, anchor: &mut Anchor, others: &[(NodeId, String)]) {
    let name = |of: NodeId| others.iter().find(|(o, _)| *o == of).map_or("?", |(_, l)| l.as_str()).to_string();
    let row = ui.horizontal(|ui| {
        let label = ui.label("Node");
        egui::ComboBox::from_id_salt("anchor")
            .selected_text(name(anchor.to))
            .show_ui(ui, |ui| {
                for (other, label) in others {
                    ui.selectable_value(&mut anchor.to, *other, label);
                }
            })
            .response
            .labelled_by(label.id);
    });
    leaf(row.response.rect);
    radio(ui, &mut anchor.at, Position::Front, "At the front");
    radio(ui, &mut anchor.at, Position::Back, "At the back");
}

/// Enabling or disabling, as a node or a connection found has it: doing nothing, enabling, or disabling.
fn enable_or_disable(ui: &mut egui::Ui, choice: &mut EnableOrDisable) {
    radio(ui, choice, EnableOrDisable::DoNothing, DO_NOTHING);
    radio(ui, choice, EnableOrDisable::Enable, "Enable");
    radio(ui, choice, EnableOrDisable::Disable, "Disable");
}

/// Beside "Use default location": where that is (see `Layout::place`).
const DEFAULT_LOCATION: &str = "The app places a new window by how many groups of tabs are showing. With one group, \
the window gets a new column to the right of it. With two groups, the window goes below the right-hand group. With \
three or more, the window becomes a tab in the largest group.";

/// Beside "Apply mode": where the mode comes from, and what's left alone.
const APPLYING_MODE: &str = "The base directory's mode is the one the app has when this is chosen. Its directory and \
anything else chosen for it are left as they are. To apply another mode, choose it in the app, then choose this again.";

/// Beside "Apply settings": where the values applied come from.
const APPLYING: &str = "A setting ticked is applied with the value it has, when ticked, on the node matched in the graph \
now. With no match in the graph, which settings are applied can't be changed.";

/// `to` with the setting named `key` as `from` has it (both nodes of the same type), if that can be read.
fn copy_setting<N: NodesEnum>(to: &mut N, from: &N, key: &str) {
    let (Ok(mut json), Ok(source)) = (serde_json::to_value(&*to), serde_json::to_value(from)) else { return };
    let Some(value) = source.get("settings").and_then(|s| s.get(key)) else { return };
    if let Some(settings) = json.get_mut("settings").and_then(|s| s.as_object_mut()) {
        settings.insert(key.to_string(), value.clone());
    }
    if let Ok(node) = serde_json::from_value(json) {
        *to = node;
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
fn connection_diagram(
    ui: &mut egui::Ui,
    from: &(String, egui::Color32, String),
    to: &(String, egui::Color32, String),
    colours: &Colours,
    paint: impl FnOnce(&egui::Painter, egui::Pos2, egui::Pos2),
) {
    // Room at the right of the nodes for the wire, looping out from one port to the other.
    const LOOP: f32 = 40.0;
    under_bar(ui, |ui| {
        let width = ui.available_width() - LOOP;
        let node = |ui: &mut egui::Ui, (label, colour, port): &(String, egui::Color32, String)| {
            let frame = egui::Frame::new()
                .fill(colours.surface_raised)
                .stroke(egui::Stroke::new(1.0, colours.border))
                .corner_radius(4.0)
                .inner_margin(egui::Margin { left: 8, right: 12, top: 5, bottom: 5 });
            let shown = frame.show(ui, |ui| {
                ui.set_width(width - 20.0);
                // The port first, at the right, so a long name is cut short before the port (and shown whole on
                // hover) rather than running into it.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(port).color(colours.text_primary));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(super::docking::tab_title(label, *colour)).truncate());
                        });
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
}

/// Under its tab bar, what the inspector shows scrolls, if it's taller or wider than the room left.
fn scrolled(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    // Both ways: a long name, as in a connection's "Else, swap to …", widens the content past the pane.
    let width = ui.available_width();
    egui::ScrollArea::both().id_salt("inspector").auto_shrink([false, false]).show(ui, |ui| {
        // The pane's width for what fills or wraps, as scrolling sideways leaves it unbounded.
        ui.set_max_width(width);
        egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 12, top: 6, bottom: 10 }).show(ui, add);
    });
}

/// A pane's tab bar, as the app's docked panes have (egui_tiles' look, which the app keeps): on the bar's colour, the
/// `tabs` (each a title, and whether it can be closed), the one at `active` at the front. This returns a tab clicked,
/// or closed.
fn tab_bar(ui: &mut egui::Ui, tabs: &[(egui::WidgetText, bool)], active: usize) -> Option<TabEvent> {
    let visuals = ui.visuals().clone();
    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), TAB_BAR), egui::Sense::hover());
    ui.painter().rect_filled(bar, 0.0, tab_bar_colour(&visuals));
    let line = egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    let mut event = None;
    let mut x = bar.left();
    let mut front = None;
    for (index, (title, closable)) in tabs.iter().enumerate() {
        let name = title.text().to_string();
        let galley = title.clone().into_galley(ui, Some(egui::TextWrapMode::Truncate), 180.0, egui::TextStyle::Button);
        // Room for a close button, as a docked pane's tab has, 4 points after the title.
        let close_room = if *closable { 4.0 + CLOSE } else { 0.0 };
        let tab = egui::Rect::from_min_size(
            egui::pos2(x, bar.top()),
            egui::vec2(galley.size().x + 16.0 + close_room, bar.height()),
        );
        x = tab.right();
        let id = ui.scope_id().with(("tab", index));
        let response = ui.interact(tab, id, egui::Sense::click());
        let is_active = index == active;
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, is_active, &name));
        let painter = ui.painter();
        if is_active {
            painter.rect_filled(tab, 0.0, visuals.panel_fill);
            let outline = egui::Stroke::new(1.0, visuals.widgets.active.bg_fill);
            painter.line_segment([tab.left_bottom(), tab.left_top()], outline);
            painter.line_segment([tab.left_top(), tab.right_top()], outline);
            painter.line_segment([tab.right_top(), tab.right_bottom()], outline);
            front = Some(tab);
        }
        let ink =
            if is_active { visuals.widgets.active.text_color() } else { visuals.widgets.noninteractive.text_color() };
        let at = egui::pos2(tab.left() + 8.0, tab.center().y - galley.size().y / 2.0);
        painter.galley(at, galley, ink);
        if response.clicked() {
            event = Some(TabEvent::Clicked(index));
        }
        // The close button, a cross, as egui_tiles draws a docked pane's.
        if *closable {
            let button = egui::Align2::RIGHT_CENTER
                .align_size_within_rect(egui::Vec2::splat(CLOSE), tab.shrink2(egui::vec2(8.0, 0.0)));
            let response = ui.interact(button, id.with("close"), egui::Sense::click());
            let label = format!("Close {name}");
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &label));
            let look = ui.style().interact(&response);
            let cross = button.shrink(2.0).expand(look.expansion);
            ui.painter().line_segment([cross.left_top(), cross.right_bottom()], look.fg_stroke);
            ui.painter().line_segment([cross.right_top(), cross.left_bottom()], look.fg_stroke);
            if response.clicked() {
                event = Some(TabEvent::Closed);
            }
        }
    }
    // The line under the bar, broken by the tab at the front.
    match front {
        Some(tab) => {
            ui.painter().hline(bar.left()..=tab.left(), bar.bottom() - 0.5, line);
            ui.painter().hline(tab.right()..=bar.right(), bar.bottom() - 0.5, line);
        }
        None => {
            ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, line);
        }
    }
    event
}

/// A tab bar's tab clicked (by index), or its closable tab closed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TabEvent {
    Clicked(usize),
    Closed,
}

/// What's selected, as the inspector's second tab shows it (see `Editor::selected`).
struct Selected {
    title: egui::WidgetText,
    used: bool,
    mixed: bool,
    /// A connection with a node at either end ignored is ignored too, and can't be used until both are.
    locked: bool,
}

/// A tab's close button is this big, as a docked pane's is.
const CLOSE: f32 = 12.0;

/// The power button, as a node's window has, for whether the quick action uses what's shown (or, `mixed`, some of
/// it); this returns whether it was clicked.
fn use_button(ui: &mut egui::Ui, used: &mut bool, mixed: bool, label: &str) -> bool {
    let tip = match (mixed, *used) {
        (true, _) => "Some used: click to use them all",
        (false, true) => "Used: click to ignore",
        (false, false) => "Ignored: click to use",
    };
    let button = toolbar::small_toggle_button(ui, Icon::Power, label, tip, *used);
    if mixed {
        toolbar::mark_mixed(ui, &button);
    }
    if button.clicked() {
        *used = mixed || !*used;
    }
    button.clicked()
}

/// The colour of a tab bar, as egui_tiles has it for the app's docked panes.
fn tab_bar_colour(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        visuals.extreme_bg_color
    } else {
        (egui::Rgba::from(visuals.panel_fill) * egui::Rgba::from_gray(0.8)).into()
    }
}

/// A tab bar's this tall, as the docked panes' are.
const TAB_BAR: f32 = 24.0;

/// Under the inspector's tab bar, what it shows of a connection, or of several nodes, before its options: inset, with
/// a line under it, as a node's window has under its heading.
fn under_bar<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let shown = egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = shown.response.rect;
    let line = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    ui.painter().hline(rect.x_range(), rect.bottom(), line);
    shown.inner
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

/// A section's line, at `x`, from `top` to its last row (or, with none, the bottom of what's in it), a stub to each
/// row, and a dot where a section branches off.
fn paint_tree(ui: &egui::Ui, x: f32, top: f32, area: egui::Rect, rows: &[(egui::Rect, bool)], colours: &Colours) {
    let colour = colours.text_secondary.gamma_multiply(0.5);
    let stroke = egui::Stroke::new(1.5, colour);
    let painter = ui.painter();
    // The line ends at its last row, even with more nested under that row (which has a line of its own).
    let bottom = rows.last().map_or(area.bottom() - 4.0, |(rect, _)| rect.center().y);
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
    fn a_setting_ticked_takes_its_value_from_the_node_matched() {
        use crate::Nodes;
        let filter = |low: f64| {
            let json =
                serde_json::json!({ "type": "BandPassFilter", "settings": { "low_hz": low, "high_hz": 6000.0 } });
            serde_json::from_value::<Nodes>(json).unwrap()
        };
        let mut role = filter(1.0);
        copy_setting(&mut role, &filter(300.0), "low_hz");
        let settings = serde_json::to_value(&role).unwrap()["settings"].clone();
        assert_eq!(settings["low_hz"], 300.0);
        assert_eq!(settings["high_hz"], 6000.0, "only the one ticked");
    }

    #[test]
    fn quick_actions_are_imported_from_files_into_the_folder() {
        use crate::Nodes;
        let dir = std::env::temp_dir().join(format!("ephys-rs-imported-quick-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (from, folder) = (dir.join("from"), dir.join("quick_actions"));
        std::fs::create_dir_all(&from).unwrap();
        let text = crate::components::quick_actions::QUICK_ACTIONS[0];
        std::fs::write(from.join("a.json"), text).unwrap();
        std::fs::write(from.join("b.json"), "not a quick action").unwrap();
        let actions = QuickActions::<Nodes> { folder: Some(folder.clone()), ..Default::default() };
        let patches = PatchSets::default();
        let (imported, problems) = actions.import(&[from.join("a.json"), from.join("b.json")], &patches);
        assert_eq!(imported, 1);
        assert!(problems.len() == 1 && problems[0].starts_with("b.json"), "{problems:?}");
        let copied = folder.join("standard-ltx-graph-v0-5.quick-action.json");
        assert_eq!(std::fs::read_to_string(copied).unwrap(), text, "as it was");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
