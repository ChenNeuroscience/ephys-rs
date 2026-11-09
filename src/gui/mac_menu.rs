//! The menu bar at the top of the screen on macOS, where apps are expected to have one, in place of the
//! window's own. Built from the same menus. Shortcuts are left to the app (as on other platforms), except Quit.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use super::actions::{self, Action, MENUS, MenuEntry};
use super::preferences::MOST_RECENT_FILES;
use crate::theme::Theme;

/// The menu bar, while it's shown: what's clicked comes back as `Action`s, and the checkmarks and recent files are
/// kept in step with the app.
pub struct MacMenu {
    /// Kept alive for as long as the menu is shown.
    _menu: Menu,
    actions: mpsc::Receiver<Action>,
    /// Items with a checkmark, to keep in step with the app.
    checks: Vec<(Action, CheckMenuItem)>,
    /// The recent files' submenu, and the files in it (none shown yet, if `None`).
    recent: Option<Submenu>,
    recent_shown: Option<Vec<PathBuf>>,
}

fn id(action: Action) -> String {
    format!("{action:?}")
}

/// What's being built: every action in the menus, and the ones with checkmarks.
struct Builder<'a> {
    actions: Vec<Action>,
    checks: Vec<(Action, CheckMenuItem)>,
    recent: Option<Submenu>,
    checkable: &'a dyn Fn(Action) -> bool,
    /// The annotation types' names, for the drawing tools.
    annotations: Vec<&'static str>,
    /// Annotation types hidden in Settings, by name: not among the drawing tools.
    hidden: &'a std::collections::BTreeSet<String>,
    themes: &'static [Theme],
}

impl Builder<'_> {
    fn item(&self, action: Action) -> MenuItem {
        let accelerator = (action == Action::Quit).then(|| "CmdOrCtrl+Q".parse().ok()).flatten();
        MenuItem::with_id(id(action), action.name(self.themes), true, accelerator)
    }

    fn items(&mut self, entries: &[MenuEntry]) -> Option<Vec<Box<dyn IsMenuItem>>> {
        let mut items: Vec<Box<dyn IsMenuItem>> = Vec::new();
        for entry in entries {
            match entry {
                // Quit lives in the app menu on macOS.
                MenuEntry::Action(Action::Quit) => {}
                MenuEntry::Action(action) => items.push(self.action(*action)),
                MenuEntry::DrawTools => {
                    let shown = self.annotations.iter().copied().filter(|name| !self.hidden.contains(*name));
                    let tools: Vec<&'static str> = shown.collect();
                    items.extend(tools.into_iter().map(|name| self.action(Action::DrawTool(name))));
                }
                MenuEntry::Themes => items.extend((0..self.themes.len()).map(|i| self.action(Action::Theme(i)))),
                // Filled in by `show_recent`.
                MenuEntry::RecentFiles => self.actions.extend((0..MOST_RECENT_FILES).map(Action::OpenRecent)),
                MenuEntry::Separator => items.push(Box::new(PredefinedMenuItem::separator())),
                MenuEntry::Submenu(title, inner) => items.push(Box::new(self.submenu(title, inner)?)),
            }
        }
        Some(items)
    }

    fn action(&mut self, action: Action) -> Box<dyn IsMenuItem> {
        self.actions.push(action);
        if (self.checkable)(action) {
            let name = action.name(self.themes);
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
        if entries.iter().any(|entry| matches!(entry, MenuEntry::RecentFiles)) {
            self.recent = Some(submenu.clone());
        }
        Some(submenu)
    }
}

impl MacMenu {
    /// Installs the menu bar. Call once the app's window exists. `checkable` says which actions are on or off.
    pub fn install(
        ctx: &egui::Context,
        checkable: &dyn Fn(Action) -> bool,
        annotations: Vec<&'static str>,
        hidden: &std::collections::BTreeSet<String>,
        themes: &'static [Theme],
    ) -> Option<Self> {
        let mut builder = Builder {
            actions: vec![Action::Quit],
            checks: Vec::new(),
            recent: None,
            checkable,
            annotations,
            hidden,
            themes,
        };
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
        Some(MacMenu { _menu: menu, actions, checks: builder.checks, recent: builder.recent, recent_shown: None })
    }

    /// Actions chosen from the menu since last asked.
    pub fn actions(&self) -> Vec<Action> {
        self.actions.try_iter().collect()
    }

    /// Shows these recent files, if they aren't what's shown already.
    pub fn show_recent(&mut self, recent: &[PathBuf]) {
        if self.recent_shown.as_deref() == Some(recent) {
            return;
        }
        self.recent_shown = Some(recent.to_vec());
        let Some(submenu) = &self.recent else { return };
        while submenu.remove_at(0).is_some() {}
        for (index, path) in recent.iter().enumerate() {
            let name = actions::recent_name(path);
            let _ = submenu.append(&MenuItem::with_id(id(Action::OpenRecent(index)), name, true, None));
        }
        if recent.is_empty() {
            let _ = submenu.append(&MenuItem::new(actions::NO_RECENT_FILES, false, None));
        }
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
