//! The menu bar at the top of the screen on macOS, where apps are expected to have one, in place of the
//! window's own. Built from the same menus. Shortcuts are left to the app (as on other platforms), except Quit.

use std::sync::mpsc;

use eframe::egui;
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use super::actions::{Action, MENUS, MenuEntry};
use crate::core::annotation::AnnotationInfo;
use crate::core::theme::Theme;

pub struct MacMenu {
    /// Kept alive for as long as the menu is shown.
    _menu: Menu,
    actions: mpsc::Receiver<Action>,
    /// Items with a checkmark, to keep in step with the app.
    checks: Vec<(Action, CheckMenuItem)>,
}

fn id(action: Action) -> String {
    format!("{action:?}")
}

/// What's being built: every action in the menus, and the ones with checkmarks.
struct Builder<'a> {
    actions: Vec<Action>,
    checks: Vec<(Action, CheckMenuItem)>,
    checkable: &'a dyn Fn(Action) -> bool,
    annotations: &'static [AnnotationInfo],
    themes: &'static [Theme],
}

impl Builder<'_> {
    fn item(&self, action: Action) -> MenuItem {
        let accelerator = (action == Action::Quit).then(|| "CmdOrCtrl+Q".parse().ok()).flatten();
        MenuItem::with_id(id(action), action.name(self.annotations, self.themes), true, accelerator)
    }

    fn items(&mut self, entries: &[MenuEntry]) -> Option<Vec<Box<dyn IsMenuItem>>> {
        let mut items: Vec<Box<dyn IsMenuItem>> = Vec::new();
        for entry in entries {
            match entry {
                // Quit lives in the app menu on macOS.
                MenuEntry::Action(Action::Quit) => {}
                MenuEntry::Action(action) => items.push(self.action(*action)),
                MenuEntry::DrawTools => {
                    items.extend((0..self.annotations.len()).map(|i| self.action(Action::DrawTool(i))));
                }
                MenuEntry::Themes => items.extend((0..self.themes.len()).map(|i| self.action(Action::Theme(i)))),
                MenuEntry::Separator => items.push(Box::new(PredefinedMenuItem::separator())),
                MenuEntry::Submenu(title, inner) => items.push(Box::new(self.submenu(title, inner)?)),
            }
        }
        Some(items)
    }

    fn action(&mut self, action: Action) -> Box<dyn IsMenuItem> {
        self.actions.push(action);
        if (self.checkable)(action) {
            let name = action.name(self.annotations, self.themes);
            let check = CheckMenuItem::with_id(id(action), name, true, false, None);
            self.checks.push((action, check.clone()));
            Box::new(check)
        } else {
            Box::new(self.item(action))
        }
    }

    fn submenu(&mut self, title: &str, entries: &[MenuEntry]) -> Option<Submenu> {
        let submenu = Submenu::new(title, true);
        let items = self.items(entries)?;
        let refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i.as_ref()).collect();
        submenu.append_items(&refs).ok()?;
        Some(submenu)
    }
}

impl MacMenu {
    /// Installs the menu bar. Call once the app's window exists. `checkable` says which actions are on or off.
    pub fn install(
        ctx: &egui::Context,
        checkable: &dyn Fn(Action) -> bool,
        annotations: &'static [AnnotationInfo],
        themes: &'static [Theme],
    ) -> Option<Self> {
        let mut builder = Builder { actions: vec![Action::Quit], checks: Vec::new(), checkable, annotations, themes };
        let app = Submenu::with_items(
            "ephys-rs",
            true,
            &[
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &builder.item(Action::Quit),
            ],
        )
        .ok()?;
        let menu = Menu::with_items(&[&app]).ok()?;
        for (title, entries) in MENUS {
            menu.append(&builder.submenu(title, entries)?).ok()?;
        }
        menu.init_for_nsapp();

        let (sender, actions) = mpsc::channel();
        let ctx = ctx.clone();
        let ids = builder.actions;
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = ids.iter().find(|a| id(**a) == event.id.0) {
                let _ = sender.send(*action);
                ctx.request_repaint();
            }
        }));
        Some(MacMenu { _menu: menu, actions, checks: builder.checks })
    }

    /// Actions chosen from the menu since last asked.
    pub fn actions(&self) -> Vec<Action> {
        self.actions.try_iter().collect()
    }

    /// Updates the checkmarks to match the app. (The menu toggles an item's checkmark itself when it's
    /// chosen, which isn't always right, e.g. choosing the theme already in use.)
    pub fn sync(&self, checked: impl Fn(Action) -> bool) {
        for (action, item) in &self.checks {
            let on = checked(*action);
            if item.is_checked() != on {
                item.set_checked(on);
            }
        }
    }
}
