//! The menu bar at the top of the screen on macOS, where apps are expected to have one, in place of the
//! window's own. Built from the same menus. Shortcuts are left to the app (as on other platforms), except Quit.

use std::sync::mpsc;

use eframe::egui;
use muda::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use super::actions::{Action, MENUS, MenuEntry};

pub struct MacMenu {
    /// Kept alive for as long as the menu is shown.
    _menu: Menu,
    actions: mpsc::Receiver<Action>,
}

fn item(action: Action) -> MenuItem {
    let accelerator = (action == Action::Quit).then(|| "CmdOrCtrl+Q".parse().ok()).flatten();
    MenuItem::with_id(format!("{action:?}"), action.name(), true, accelerator)
}

impl MacMenu {
    /// Installs the menu bar. Call once the app's window exists.
    pub fn install(ctx: &egui::Context) -> Option<Self> {
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
                &item(Action::Quit),
            ],
        )
        .ok()?;
        let menu = Menu::with_items(&[&app]).ok()?;
        let mut ids = Vec::new();
        for (title, entries) in MENUS {
            let submenu = Submenu::new(*title, true);
            // Quit lives in the app menu on macOS.
            let entries = entries.iter().filter(|e| !matches!(e, MenuEntry::Action(Action::Quit)));
            let items: Vec<Box<dyn IsMenuItem>> = entries
                .map(|entry| -> Box<dyn IsMenuItem> {
                    match entry {
                        MenuEntry::Action(action) => {
                            ids.push(*action);
                            Box::new(item(*action))
                        }
                        MenuEntry::Separator => Box::new(PredefinedMenuItem::separator()),
                    }
                })
                .collect();
            let refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i.as_ref()).collect();
            submenu.append_items(&refs).ok()?;
            menu.append(&submenu).ok()?;
        }
        menu.init_for_nsapp();

        ids.push(Action::Quit);
        let (sender, actions) = mpsc::channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = ids.iter().find(|a| format!("{a:?}") == event.id.0) {
                let _ = sender.send(*action);
                ctx.request_repaint();
            }
        }));
        Some(MacMenu { _menu: menu, actions })
    }

    /// Actions chosen from the menu since last asked.
    pub fn actions(&self) -> Vec<Action> {
        self.actions.try_iter().collect()
    }
}
