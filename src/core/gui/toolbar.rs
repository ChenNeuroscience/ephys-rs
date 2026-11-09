//! The menu bar and toolbar along the top of the window, in the style of desktop apps before ribbons: menus
//! of every action, then a row of small flat icon buttons for the common ones. The graph has a toolbar of its own,
//! along the top of its pane, for what's done to it (`graph_toolbar`).

use eframe::egui::{self, Rect, Sense, Stroke, StrokeKind};

use super::actions::{self, Action, MENUS, MenuEntry};
use super::canvas::Tool;
use super::icons::{self, Icon};
use super::{App, Event};
use crate::core::annotation::{AnnotationTypeEnum, AnnotationsEnum};
use crate::core::base_directory::{BaseDirectoryContext, BaseDirectoryType};
use crate::core::engine::NodesEnum;
use crate::core::layout::{Mode, Pane};

const BUTTON: egui::Vec2 = egui::vec2(26.0, 24.0);

/// `text`, with the action's shortcut, for a tooltip.
fn tooltip(ctx: &egui::Context, action: Action, text: &str) -> String {
    match action.shortcut() {
        Some(shortcut) => format!("{text} ({})", ctx.format_shortcut(&shortcut)),
        None => text.to_string(),
    }
}

/// Which of the graph's toolbar's actions can be done right now.
#[derive(Clone, Copy)]
pub struct GraphTools {
    pub add: bool,
    pub duplicate: bool,
    pub delete: bool,
    pub arrange: bool,
}

/// The graph's toolbar, along the top of its pane: adding nodes, and drawing annotations (those not `hidden`, by
/// name); duplicating and deleting; arranging and framing it.
pub fn graph_toolbar<A: AnnotationsEnum>(
    ui: &mut egui::Ui,
    tools: GraphTools,
    tool: Tool,
    hidden: &std::collections::BTreeSet<String>,
    events: &mut Vec<Event>,
) {
    let ctx = ui.ctx().clone();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(BUTTON.y);
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut perform = |action, anchor| events.push(Event::Perform(action, anchor));
            let add = dropdown_button(ui, Icon::AddNode, "Add node")
                .on_hover_text("Add a node (or press Tab over the graph)");
            if add.clicked() && tools.add {
                perform(Action::AddNode, Some(add.rect.left_bottom()));
            }
            // A tool for drawing each type of annotation.
            for &annotation_type in A::Type::ALL.iter().filter(|t| !hidden.contains(t.info().name)) {
                let info = annotation_type.info();
                let tip = format!("{}: {}", info.name, info.description);
                let paint = |painter: &egui::Painter, rect: Rect, _: egui::Color32, _: bool| {
                    annotation_type.paint_icon(painter, rect)
                };
                if icon_button(ui, &paint, &tip, &tip, BUTTON, 16.0, true, tool == Tool::Draw(info.name)).clicked() {
                    perform(Action::DrawTool(info.name), None);
                }
            }
            ui.separator();
            for (action, icon, text, enabled) in [
                (Action::Duplicate, Icon::Duplicate, "Duplicate", tools.duplicate),
                (Action::Delete, Icon::Delete, "Delete", tools.delete),
            ] {
                if tool_button(ui, icon, &tooltip(&ctx, action, text), enabled, false).clicked() {
                    perform(action, None);
                }
            }
            ui.separator();
            for (action, icon, text, enabled) in [
                (
                    Action::Arrange,
                    Icon::Arrange,
                    "Arrange nodes: in columns, left to right the way data flows",
                    tools.arrange,
                ),
                (Action::FrameAll, Icon::FrameAll, "Frame all", true),
                (Action::ActualSize, Icon::ActualSize, "Actual size", true),
            ] {
                if tool_button(ui, icon, &tooltip(&ctx, action, text), enabled, false).clicked() {
                    perform(action, None);
                }
            }
        });
    });
}

impl<N: NodesEnum, A: AnnotationsEnum> App<N, A> {
    /// Whether an action can be done right now.
    pub(super) fn enabled(&self, action: Action) -> bool {
        let selected = !self.selection.nodes.is_empty() || !self.selection.annotations.is_empty();
        // While recording, only annotations can change.
        let locked = self.locked();
        let only_annotations = self.selection.nodes.is_empty() && self.selection.edge.is_none();
        match action {
            Action::Undo => self.history.can_undo() && !locked,
            Action::Redo => self.history.can_redo() && !locked,
            Action::Duplicate => selected && (!locked || only_annotations),
            Action::Delete => (selected || self.selection.edge.is_some()) && (!locked || only_annotations),
            Action::AddNode => !locked,
            Action::Arrange => self.graph.iter().count() > 1,
            Action::CloseHome => self.layout.is_shown(Pane::Home),
            Action::CopySettings => !self.selection.nodes.is_empty(),
            Action::ToggleDisabled => !self.selection.nodes.is_empty() || self.selection.edge.is_some(),
            _ => true,
        }
    }

    fn tooltip(&self, ctx: &egui::Context, action: Action, text: &str) -> String {
        tooltip(ctx, action, text)
    }

    /// Which of the graph's toolbar's actions can be done right now.
    pub(super) fn graph_tools(&self) -> GraphTools {
        GraphTools {
            add: self.enabled(Action::AddNode),
            duplicate: self.enabled(Action::Duplicate),
            delete: self.enabled(Action::Delete),
            arrange: self.enabled(Action::Arrange),
        }
    }

    /// For actions that are on or off (a panel showing, the theme in use), whether they're on.
    pub(super) fn checked(&self, action: Action) -> Option<bool> {
        match action {
            Action::DrawTool(name) => Some(self.canvas.tool == Tool::Draw(name)),
            Action::ToggleDevTools => Some(self.layout.is_shown(Pane::DevTools)),
            Action::Advanced => Some(self.layout.mode == Mode::Advanced),
            Action::Theme(index) => Some(self.theme == index),
            Action::Play => Some(self.runner.is_some()),
            Action::Record => Some(self.history.current.snapshot.recording.is_some() || self.preparing.is_some()),
            _ => None,
        }
    }

    /// The menus, in the window. Returns the action chosen, if any.
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut chosen = None;
        egui::MenuBar::new().ui(ui, |ui| {
            for (title, entries) in MENUS {
                ui.menu_button(*title, |ui| self.menu_entries(ui, entries, &mut chosen));
            }
        });
        chosen
    }

    fn menu_entries(&self, ui: &mut egui::Ui, entries: &[MenuEntry], chosen: &mut Option<Action>) {
        for entry in entries {
            match entry {
                MenuEntry::Separator => {
                    ui.separator();
                }
                MenuEntry::Submenu(title, inner) => {
                    ui.menu_button((check(false), *title), |ui| self.menu_entries(ui, inner, chosen));
                }
                MenuEntry::Action(action) => self.menu_item(ui, *action, chosen),
                MenuEntry::DrawTools => {
                    let names = A::Type::ALL.iter().map(|t| t.info().name);
                    for name in names.filter(|name| !self.hidden.annotations.contains(*name)) {
                        self.menu_item(ui, Action::DrawTool(name), chosen);
                    }
                }
                MenuEntry::Themes => {
                    for index in 0..self.themes.len() {
                        self.menu_item(ui, Action::Theme(index), chosen);
                    }
                }
                MenuEntry::RecentFiles => {
                    if self.recent.is_empty() {
                        ui.add_enabled(false, egui::Button::new(actions::NO_RECENT_FILES));
                    }
                    for (index, path) in self.recent.iter().enumerate() {
                        if ui.button(actions::recent_name(path)).clicked() {
                            *chosen = Some(Action::OpenRecent(index));
                        }
                    }
                }
            }
        }
    }

    fn menu_item(&self, ui: &mut egui::Ui, action: Action, chosen: &mut Option<Action>) {
        let on = self.checked(action) == Some(true);
        let mut button = egui::Button::new((check(on), action.name(self.themes)));
        if let Some(shortcut) = action.shortcut() {
            button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
        }
        if ui.add_enabled(self.enabled(action), button).clicked() {
            *chosen = Some(action);
        }
    }

    /// The row of icon buttons. Returns the action chosen, if any.
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let ctx = ui.ctx().clone();
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let groups: [&[(Action, Icon, &str)]; 2] = [
                &[
                    (Action::New, Icon::New, "New graph"),
                    (Action::Open, Icon::Open, "Open"),
                    (Action::Save, Icon::Save, "Save"),
                ],
                &[(Action::Undo, Icon::Undo, "Undo"), (Action::Redo, Icon::Redo, "Redo")],
            ];
            for group in groups {
                for &(action, icon, text) in group {
                    let tip = self.tooltip(&ctx, action, text);
                    if tool_button(ui, icon, &tip, self.enabled(action), false).clicked() {
                        chosen = Some(action);
                    }
                }
                ui.separator();
            }

            // Home, and advanced mode (the graph in it, and panes' settings).
            let home = if self.layout.mode == Mode::Basic { "the basic view" } else { "the graph" };
            let tip = self.tooltip(&ctx, Action::Home, &format!("Home: {home}"));
            if tool_button(ui, Icon::Home, &tip, true, false).clicked() {
                chosen = Some(Action::Home);
            }
            let advanced = self.layout.mode == Mode::Advanced;
            let tip = self.tooltip(
                &ctx,
                Action::Advanced,
                "Advanced mode: the graph, to build and change it node by node, and each window's settings",
            );
            if tool_button(ui, Icon::Graph, &tip, true, advanced).clicked() {
                chosen = Some(Action::Advanced);
            }
            ui.separator();

            let running = self.runner.is_some();
            let (icon, text) = if running { (Icon::Stop, "Stop") } else { (Icon::Play, "Play") };
            let tip = self.tooltip(&ctx, Action::Play, text);
            if tool_button(ui, icon, &tip, true, running).clicked() {
                chosen = Some(Action::Play);
            }
            self.play_clock(ui);
            ui.add_space(6.0);
            let recording = self.history.current.snapshot.recording.is_some();
            let text = if recording {
                "Stop recording"
            } else if self.preparing.is_some() {
                "Preparing to record (the writers opening their files): click to cancel"
            } else {
                "Record (starting the graph, if it isn't running)"
            };
            let tip = self.tooltip(&ctx, Action::Record, text);
            if tool_button(ui, Icon::Record, &tip, true, recording || self.preparing.is_some()).clicked() {
                chosen = Some(Action::Record);
            }
            self.record_clock(ui, recording);
            ui.separator();
            if self.base_directory_ui(ui) {
                ui.separator();
                if let Some(place) = self.base_directories.place() {
                    self.disk_space.ui(ui, &place.directory, self.colours());
                    ui.separator();
                }
            }
            let tip = self.tooltip(&ctx, Action::ToggleDevTools, "Developer tools");
            if tool_button(ui, Icon::DevTools, &tip, true, self.layout.is_shown(Pane::DevTools)).clicked() {
                chosen = Some(Action::ToggleDevTools);
            }
            let button = tool_button(ui, Icon::Theme, "Theme", true, false);
            egui::Popup::menu(&button).show(|ui| {
                for (index, theme) in self.themes.iter().enumerate() {
                    if ui.add(egui::Button::new((check(self.theme == index), theme.name))).clicked() {
                        chosen = Some(Action::Theme(index));
                    }
                }
            });
        });
        chosen
    }
}

impl<N: NodesEnum, A: AnnotationsEnum> App<N, A> {
    /// Where recordings go: the base directory in use's section (see `crate::core::base_directory`), then in advanced
    /// mode, choosing which is in use. Whether there's one.
    fn base_directory_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let types = self.base_directories.types();
        if types.is_empty() {
            return false;
        }
        let colours = self.colours();
        let recording = self.history.current.snapshot.recording.is_some() || self.preparing.is_some();
        if let Some(current) = self.base_directories.current() {
            current.toolbar_ui(ui, &BaseDirectoryContext { theme: colours, recording });
        }
        if self.layout.mode == Mode::Advanced && types.len() > 1 {
            let tip = if recording { "Where recordings go: not while recording" } else { "Where recordings go" };
            let button = tool_button(ui, Icon::Settings, tip, !recording, false);
            let chosen = self.base_directories.chosen();
            egui::Popup::menu(&button).show(|ui| {
                // Those hidden in Settings aren't offered, unless one's in use.
                let offered = |&(index, kind): &(usize, &BaseDirectoryType)| {
                    chosen == Some(index) || !self.hidden.base_directories.contains(kind.name)
                };
                for (index, kind) in types.iter().enumerate().filter(offered) {
                    let item = egui::Button::new((check(chosen == Some(index)), kind.name));
                    if ui.add(item).on_hover_text(kind.description).clicked() {
                        self.base_directories.choose(index);
                    }
                }
            });
        }
        true
    }

    /// The acquisition clock: how long the graph's been running, greyed out when it isn't.
    fn play_clock(&self, ui: &mut egui::Ui) {
        let colours = self.colours();
        let clock = self.runner.as_ref().map(|r| r.clock());
        let text = egui::RichText::new(super::clock_text(clock.unwrap_or(0.0))).monospace();
        let text = if clock.is_some() { text.color(colours.text_primary) } else { text.color(colours.text_disabled) };
        ui.add_space(2.0);
        ui.label(text).on_hover_text("Acquisition time: how long the graph has been running");
        if clock.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// How long it's been recording, greyed out when it isn't (and saying so while getting ready to).
    fn record_clock(&self, ui: &mut egui::Ui, recording: bool) {
        let colours = self.colours();
        let since =
            if recording { self.runner.as_ref().map_or(0.0, |r| r.clock()) - self.recording_since } else { 0.0 };
        let text = egui::RichText::new(super::clock_text(since)).monospace();
        let text = if recording { text.color(colours.error) } else { text.color(colours.text_disabled) };
        ui.add_space(2.0);
        ui.label(text).on_hover_text("How long it's been recording");
        if self.preparing.is_some() {
            ui.label(egui::RichText::new("preparing…").color(colours.text_secondary));
        }
    }
}

/// A menu item's checkmark, as in a native menu: ticked when `on`, otherwise blank but taking the same space,
/// so the items' names line up.
fn check(on: bool) -> egui::Atom<'static> {
    egui::Atom::paint(egui::vec2(12.0, 12.0), move |ui, args| {
        if on {
            let r = args.rect.shrink(1.5);
            let points = vec![
                egui::pos2(r.left(), r.center().y),
                egui::pos2(r.left() + r.width() * 0.38, r.bottom()),
                egui::pos2(r.right(), r.top()),
            ];
            ui.painter().add(egui::Shape::line(points, Stroke::new(1.6, args.fallback_text_color)));
        }
    })
}

/// A flat icon button, as along the main toolbar (for sections of it drawn by components: see
/// `crate::core::base_directory`).
pub fn toolbar_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str, enabled: bool) -> egui::Response {
    tool_button(ui, icon, tooltip, enabled, false)
}

/// A flat icon button, framed only when hovered or `selected` (a tool that's in use).
fn tool_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str, enabled: bool, selected: bool) -> egui::Response {
    icon_button(ui, &|p, r, ink, on| icon.paint(p, r, ink, on), tooltip, tooltip, BUTTON, 16.0, enabled, selected)
}

/// A smaller flat icon button, for tab bars and window corners. `label` is its accessible name.
pub fn small_icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, tooltip: &str) -> egui::Response {
    icon_button(
        ui,
        &|p, r, ink, on| icon.paint(p, r, ink, on),
        label,
        tooltip,
        egui::vec2(20.0, 20.0),
        13.0,
        true,
        false,
    )
}

/// A small icon button that's on or off (shown selected while on).
pub fn small_toggle_button(ui: &mut egui::Ui, icon: Icon, label: &str, tooltip: &str, on: bool) -> egui::Response {
    icon_button(
        ui,
        &|p, r, ink, enabled| icon.paint(p, r, ink, enabled),
        label,
        tooltip,
        egui::vec2(22.0, 22.0),
        14.0,
        true,
        on,
    )
}

/// Paints an icon to fill a rect, given the ink colour and whether it's enabled.
type PaintIcon<'a> = &'a dyn Fn(&egui::Painter, Rect, egui::Color32, bool);

#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &mut egui::Ui,
    paint: PaintIcon,
    label: &str,
    tooltip: &str,
    size: egui::Vec2,
    icon_size: f32,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    frame(ui, rect, &response, enabled, selected);
    let icon_rect = Rect::from_center_size(rect.center(), egui::vec2(icon_size, icon_size));
    paint(ui.painter(), icon_rect, ui.visuals().text_color(), enabled);
    response.on_hover_text(tooltip)
}

/// An icon, a label and a down arrow, for a button that opens a menu.
fn dropdown_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, ui.visuals().text_color());
    let width = 6.0 + 16.0 + 5.0 + galley.size().x + 6.0 + 8.0 + 6.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, BUTTON.y), Sense::click());
    frame(ui, rect, &response, true, false);
    let painter = ui.painter();
    let icon_rect = Rect::from_min_size(egui::pos2(rect.left() + 6.0, rect.center().y - 8.0), egui::vec2(16.0, 16.0));
    icon.paint(painter, icon_rect, ui.visuals().text_color(), true);
    let text_pos = egui::pos2(icon_rect.right() + 5.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley, ui.visuals().text_color());
    icons::dropdown_arrow(painter, egui::pos2(rect.right() - 10.0, rect.center().y), ui.visuals().text_color());
    response
}

fn frame(ui: &egui::Ui, rect: Rect, response: &egui::Response, enabled: bool, selected: bool) {
    let visuals = ui.visuals();
    let (fill, stroke) = if enabled && response.is_pointer_button_down_on() {
        (visuals.widgets.active.weak_bg_fill, visuals.widgets.active.bg_stroke)
    } else if selected {
        (visuals.selection.bg_fill.gamma_multiply(0.5), Stroke::new(1.0, visuals.selection.bg_fill))
    } else if enabled && response.hovered() {
        (visuals.widgets.hovered.weak_bg_fill, visuals.widgets.hovered.bg_stroke)
    } else {
        return;
    };
    ui.painter().rect(rect, 3.0, fill, stroke, StrokeKind::Inside);
}
