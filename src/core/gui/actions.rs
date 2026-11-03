//! The app's commands, with their names, shortcuts and place in the menus. The menu bar (in the window, or
//! on macOS the system's), the toolbar and the keyboard shortcuts all work from these.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

use crate::core::annotation::AnnotationInfo;
use crate::core::theme::Theme;

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
    /// Lays the nodes out in columns, left to right the way data flows.
    Arrange,
    /// Copies the selected nodes' settings to the clipboard.
    CopySettings,
    /// Opens the window to paste settings from the clipboard onto nodes.
    PasteSettings,
    /// Disables the selected nodes (or edge), or enables them if they're all disabled.
    ToggleDisabled,
    /// Opens the menu of node types.
    AddNode,
    /// Picks the tool for drawing annotations of a type (by index) on the graph.
    DrawTool(usize),
    /// Shows the home pane: the basic view, or in advanced mode the graph.
    Home,
    /// Turns advanced mode on or off: the graph in the home pane, and panes' settings (see `Mode`).
    Advanced,
    /// Closes the home pane (Home brings it back).
    CloseHome,
    FrameAll,
    ActualSize,
    ResetLayout,
    ToggleDevTools,
    /// Chooses a theme, by index.
    Theme(usize),
    /// Starts or stops running the graph.
    Play,
    /// Starts or stops recording (starting the graph running, if it isn't).
    Record,
}

pub enum MenuEntry {
    Action(Action),
    Separator,
    Submenu(&'static str, &'static [MenuEntry]),
    /// A tool for each annotation type the program has.
    DrawTools,
    /// Each theme the program has.
    Themes,
}

use MenuEntry::{Action as Item, DrawTools, Separator, Submenu, Themes};

/// The menus, in order.
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
            Separator,
            Item(Action::ToggleDisabled),
            Item(Action::CopySettings),
            Item(Action::PasteSettings),
            Separator,
            Item(Action::Arrange),
        ],
    ),
    ("Insert", &[Item(Action::AddNode), DrawTools]),
    ("Run", &[Item(Action::Play), Item(Action::Record)]),
    (
        "View",
        &[
            Item(Action::Home),
            Item(Action::Advanced),
            Item(Action::CloseHome),
            Separator,
            Item(Action::FrameAll),
            Item(Action::ActualSize),
            Separator,
            Item(Action::ToggleDevTools),
            Item(Action::ResetLayout),
            Separator,
            Submenu("Theme", &[Themes]),
        ],
    ),
];

/// Checked in this order, so that e.g. shift-ctrl-Z (redo) is seen before ctrl-Z (undo).
const SHORTCUT_ORDER: [Action; 18] = [
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
    Action::Play,
    Action::Record,
    Action::ToggleDevTools,
    Action::CopySettings,
    Action::PasteSettings,
    Action::ToggleDisabled,
    Action::Home,
    Action::Advanced,
];

impl Action {
    /// Its name in menus. Annotation tools and themes are named from the program's `annotations` and `themes`.
    pub fn name<'a>(self, annotations: &'a [AnnotationInfo], themes: &'a [Theme]) -> &'a str {
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
            Action::Arrange => "Arrange Nodes",
            Action::CopySettings => "Copy Settings",
            Action::PasteSettings => "Paste Settings…",
            Action::ToggleDisabled => "Disable or Enable",
            Action::AddNode => "Node…",
            Action::DrawTool(index) => annotations.get(index).map_or("?", |a| a.name),
            Action::Home => "Home",
            Action::Advanced => "Advanced Mode",
            Action::CloseHome => "Close Home",
            Action::FrameAll => "Frame All",
            Action::ActualSize => "Actual Size",
            Action::ResetLayout => "Reset Layout",
            Action::ToggleDevTools => "Developer Tools",
            Action::Theme(index) => themes.get(index).map_or("?", |t| t.name),
            Action::Play => "Play",
            Action::Record => "Record",
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
            Action::Play => Some(KeyboardShortcut::new(Modifiers::NONE, Key::Space)),
            Action::Record => command(Key::R),
            Action::CopySettings => Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, Key::C)),
            Action::PasteSettings => Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, Key::V)),
            Action::ToggleDisabled => command(Key::E),
            Action::Home => command(Key::Num0),
            Action::Advanced => command(Key::Num1),
            // As in browsers.
            Action::ToggleDevTools => Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, Key::I)),
            _ => None,
        }
    }

    /// Shortcuts that would get in the way of typing, so only apply when no text field has focus.
    fn only_when_not_typing(self) -> bool {
        matches!(self, Action::Duplicate | Action::Delete | Action::SelectAll | Action::FrameAll | Action::Play)
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
