//! The app's commands, with their names, shortcuts and place in the menus. The menu bar (in the window, or
//! on macOS the system's), the toolbar and the keyboard shortcuts all work from these.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Quit,
    Undo,
    Redo,
    Duplicate,
    Delete,
    SelectAll,
    /// Opens the menu of node types.
    AddNode,
    /// Picks the tool for drawing sticky notes on the graph.
    NoteTool,
    FrameAll,
    ActualSize,
    ResetLayout,
}

pub enum MenuEntry {
    Action(Action),
    Separator,
}

use MenuEntry::{Action as Item, Separator};

/// The menus, in order. View also gets the theme and panel choices, which aren't plain actions.
pub const MENUS: &[(&str, &[MenuEntry])] = &[
    (
        "File",
        &[
            Item(Action::New),
            Item(Action::Open),
            Separator,
            Item(Action::Save),
            Item(Action::SaveAs),
            Separator,
            Item(Action::Quit),
        ],
    ),
    (
        "Edit",
        &[
            Item(Action::Undo),
            Item(Action::Redo),
            Separator,
            Item(Action::Duplicate),
            Item(Action::Delete),
            Item(Action::SelectAll),
        ],
    ),
    ("Insert", &[Item(Action::AddNode), Item(Action::NoteTool)]),
    ("View", &[Item(Action::FrameAll), Item(Action::ActualSize), Separator, Item(Action::ResetLayout)]),
];

/// Checked in this order, so that e.g. shift-ctrl-Z (redo) is seen before ctrl-Z (undo).
const SHORTCUT_ORDER: [Action; 10] = [
    Action::Redo,
    Action::Undo,
    Action::SaveAs,
    Action::Save,
    Action::Open,
    Action::New,
    Action::Duplicate,
    Action::SelectAll,
    Action::Delete,
    Action::FrameAll,
];

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::New => "New",
            Action::Open => "Open…",
            Action::Save => "Save",
            Action::SaveAs => "Save As…",
            Action::Quit => {
                if cfg!(target_os = "macos") {
                    "Quit ephys-rs"
                } else {
                    "Exit"
                }
            }
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Duplicate => "Duplicate",
            Action::Delete => "Delete",
            Action::SelectAll => "Select All",
            Action::AddNode => "Node…",
            Action::NoteTool => "Sticky Note",
            Action::FrameAll => "Frame All",
            Action::ActualSize => "Actual Size",
            Action::ResetLayout => "Reset Layout",
        }
    }

    pub fn shortcut(self) -> Option<KeyboardShortcut> {
        let command = |key| Some(KeyboardShortcut::new(Modifiers::COMMAND, key));
        match self {
            Action::New => command(Key::N),
            Action::Open => command(Key::O),
            Action::Save => command(Key::S),
            Action::SaveAs => Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)),
            Action::Undo => command(Key::Z),
            Action::Redo => Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)),
            Action::Duplicate => command(Key::D),
            Action::Delete => Some(KeyboardShortcut::new(Modifiers::NONE, Key::Delete)),
            Action::SelectAll => command(Key::A),
            Action::FrameAll => Some(KeyboardShortcut::new(Modifiers::NONE, Key::F)),
            _ => None,
        }
    }

    /// Shortcuts that would get in the way of typing, so only apply when no text field has focus.
    fn only_when_not_typing(self) -> bool {
        matches!(self, Action::Duplicate | Action::Delete | Action::SelectAll | Action::FrameAll)
    }

    /// The actions whose shortcuts were pressed this frame.
    pub fn pressed(ctx: &eframe::egui::Context, typing: bool) -> Vec<Action> {
        let mut pressed = Vec::new();
        for action in SHORTCUT_ORDER {
            if typing && action.only_when_not_typing() {
                continue;
            }
            let Some(shortcut) = action.shortcut() else { continue };
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                pressed.push(action);
            }
        }
        // Alternatives to the main shortcuts.
        let alternative = |modifiers, key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if alternative(Modifiers::COMMAND, Key::Y) {
            pressed.push(Action::Redo);
        }
        if !typing && alternative(Modifiers::NONE, Key::Backspace) {
            pressed.push(Action::Delete);
        }
        pressed
    }
}
