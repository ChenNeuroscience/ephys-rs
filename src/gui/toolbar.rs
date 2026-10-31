//! The menu bar and toolbar along the top of the window, in the style of desktop apps before ribbons: menus
//! of every action, then a row of small flat icon buttons for the common ones.

use eframe::egui::{self, Rect, Sense, Stroke, StrokeKind};

use super::App;
use super::actions::{Action, MENUS, MenuEntry};
use super::canvas::Tool;
use super::docking::Pane;
use super::icons::{self, Icon};
use crate::engine::NodeSet;
use crate::theme::Theme;

const BUTTON: egui::Vec2 = egui::vec2(26.0, 24.0);

impl<N: NodeSet> App<N> {
    /// Whether an action can be done right now.
    pub(super) fn enabled(&self, action: Action) -> bool {
        let selected = !self.selection.nodes.is_empty() || !self.selection.annotations.is_empty();
        match action {
            Action::Undo => self.history.can_undo(),
            Action::Redo => self.history.can_redo(),
            Action::Duplicate => selected,
            Action::Delete => selected || self.selection.edge.is_some(),
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
            Action::NoteTool => Some(self.canvas.tool == Tool::StickyNote),
            Action::ToggleInspector => Some(self.layout.is_shown(Pane::Inspector)),
            Action::Theme(theme) => Some(self.theme == theme),
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
                MenuEntry::Action(action) => {
                    let on = self.checked(*action) == Some(true);
                    let mut button = egui::Button::new((check(on), action.name()));
                    if let Some(shortcut) = action.shortcut() {
                        button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
                    }
                    if ui.add_enabled(self.enabled(*action), button).clicked() {
                        *chosen = Some(*action);
                    }
                }
            }
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

            let add = dropdown_button(ui, Icon::AddNode, "Add node")
                .on_hover_text("Add a node (or press Tab over the graph)");
            if add.clicked() {
                self.add_anchor = Some(add.rect.left_bottom());
                chosen = Some(Action::AddNode);
            }
            let drawing = self.canvas.tool == Tool::StickyNote;
            let tip = "Sticky note: drag out on the graph to draw one (or click for the default size)";
            if tool_button(ui, Icon::Note, tip, true, drawing).clicked() {
                chosen = Some(Action::NoteTool);
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
            let button = tool_button(ui, Icon::Theme, "Theme", true, false);
            egui::Popup::menu(&button).show(|ui| {
                for theme in Theme::ALL {
                    if ui.add(egui::Button::new((check(self.theme == theme), theme.name()))).clicked() {
                        chosen = Some(Action::Theme(theme));
                    }
                }
            });
        });
        chosen
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
    icon_button(ui, icon, tooltip, tooltip, BUTTON, 16.0, enabled, selected)
}

/// A smaller flat icon button, for tab bars and window corners. `label` is its accessible name.
pub fn small_icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, tooltip: &str) -> egui::Response {
    icon_button(ui, icon, label, tooltip, egui::vec2(20.0, 20.0), 13.0, true, false)
}

#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
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
    icon.paint(ui.painter(), icon_rect, ui.visuals().text_color(), enabled);
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
