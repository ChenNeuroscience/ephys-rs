//! The menu bar and toolbar along the top of the window, in the style of desktop apps before ribbons: menus
//! of every action, then a row of small flat icon buttons for the common ones.

use eframe::egui::{self, Rect, Sense, Stroke, StrokeKind};

use super::App;
use super::actions::{Action, MENUS, MenuEntry};
use super::canvas::Tool;
use super::docking::Pane;
use super::icons::{self, Icon};
use crate::core::annotation::AnnotationSet;
use crate::core::engine::NodeSet;

const BUTTON: egui::Vec2 = egui::vec2(26.0, 24.0);

impl<N: NodeSet, A: AnnotationSet> App<N, A> {
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
            _ => true,
        }
    }

    fn tooltip(&self, ctx: &egui::Context, action: Action, text: &str) -> String {
        match action.shortcut() {
            Some(shortcut) => format!("{text} ({})", ctx.format_shortcut(&shortcut)),
            None => text.to_string(),
        }
    }

    /// For actions that are on or off (a panel showing, the theme in use), whether they're on.
    pub(super) fn checked(&self, action: Action) -> Option<bool> {
        match action {
            Action::DrawTool(index) => Some(self.canvas.tool == Tool::Draw(index)),
            Action::ToggleInspector => Some(self.layout.is_shown(Pane::Inspector)),
            Action::ToggleDevTools => Some(self.layout.is_shown(Pane::DevTools)),
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
                    for index in 0..A::TYPES.len() {
                        self.menu_item(ui, Action::DrawTool(index), chosen);
                    }
                }
                MenuEntry::Themes => {
                    for index in 0..self.themes.len() {
                        self.menu_item(ui, Action::Theme(index), chosen);
                    }
                }
            }
        }
    }

    fn menu_item(&self, ui: &mut egui::Ui, action: Action, chosen: &mut Option<Action>) {
        let on = self.checked(action) == Some(true);
        let mut button = egui::Button::new((check(on), action.name(A::TYPES, self.themes)));
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

            let add = dropdown_button(ui, Icon::AddNode, "Add node")
                .on_hover_text("Add a node (or press Tab over the graph)");
            if add.clicked() {
                self.add_anchor = Some(add.rect.left_bottom());
                chosen = Some(Action::AddNode);
            }
            // A tool for drawing each type of annotation.
            for (index, info) in A::TYPES.iter().enumerate() {
                let drawing = self.canvas.tool == Tool::Draw(index);
                let tip = format!("{}: {}", info.name, info.description);
                let paint = |painter: &egui::Painter, rect: Rect, _: egui::Color32, _: bool| {
                    A::paint_icon(index, painter, rect)
                };
                if icon_button(ui, &paint, &tip, &tip, BUTTON, 16.0, true, drawing).clicked() {
                    chosen = Some(Action::DrawTool(index));
                }
            }
            ui.separator();

            for (action, icon, text) in
                [(Action::Duplicate, Icon::Duplicate, "Duplicate"), (Action::Delete, Icon::Delete, "Delete")]
            {
                let tip = self.tooltip(&ctx, action, text);
                if tool_button(ui, icon, &tip, self.enabled(action), false).clicked() {
                    chosen = Some(action);
                }
            }
            ui.separator();
            for (action, icon, text) in
                [(Action::FrameAll, Icon::FrameAll, "Frame all"), (Action::ActualSize, Icon::ActualSize, "Actual size")]
            {
                let tip = self.tooltip(&ctx, action, text);
                if tool_button(ui, icon, &tip, true, false).clicked() {
                    chosen = Some(action);
                }
            }

            ui.separator();
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

impl<N: NodeSet, A: AnnotationSet> App<N, A> {
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
