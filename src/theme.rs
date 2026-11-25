//! Everything draws with a theme's colours, each named by what it's for (surfaces, text, controls...). The program
//! supplies the themes. egui's own widgets get them as its `Visuals`; the
//! graph and node windows use them directly (nodes get them in `engine::UiContext`), and can derive further
//! colours of their own from them.

use eframe::egui::{self, Color32, Shadow, Stroke};

/// People choose a theme by its name, and it gives its colours. The program supplies its themes (see `Components`).
pub struct Theme {
    pub name: &'static str,
    pub colours: Colours,
}

impl Theme {
    /// The app makes this the theme for all of egui (panels, widgets, menus, windows) with this.
    pub fn apply(&self, ctx: &egui::Context) {
        let base = if self.colours.dark { egui::Theme::Dark } else { egui::Theme::Light };
        ctx.set_visuals_of(base, self.colours.visuals());
        ctx.set_theme(base);
    }
}

/// A theme gives a colour for each role.
pub struct Colours {
    /// A dark theme has light text on dark surfaces; this says whether it's one.
    pub dark: bool,

    // Surfaces, from the furthest back to the closest.
    /// Behind everything is the background: the graph canvas.
    pub background: Color32,
    /// Set into the surface are text fields, plots and other views of data, and the tab bar.
    pub surface_sunken: Color32,
    /// Panels, windows and menus are on this surface.
    pub surface: Color32,
    /// Raised on top of a surface are nodes and cards.
    pub surface_raised: Color32,
    /// Lines over the background (the graph's grid, say) are in this.
    pub grid: Color32,

    pub border: Color32,
    /// Lines within a surface, between its parts, are in this.
    pub divider: Color32,

    pub text_primary: Color32,
    /// Less important text (hints, details, labels on data) is in this.
    pub text_secondary: Color32,
    pub text_disabled: Color32,

    // Buttons and other things to click or drag.
    pub control: Color32,
    pub control_hovered: Color32,
    pub control_pressed: Color32,

    /// Selection, focus, and anything that should stand out, are in the accent.
    pub accent: Color32,
    /// Text and marks on top of the accent colour are in this.
    pub on_accent: Color32,

    pub warning: Color32,
    pub error: Color32,
    pub success: Color32,
    pub shadow: Color32,

    /// The chrome (the menu bar, toolbar and status bar around the panes) can have colours of its own, a whole set
    /// of them, as it can be dark where the panes are light. With none, the chrome has the panes' colours.
    pub chrome: Option<&'static Colours>,
}

impl Colours {
    /// The chrome's colours are its own, if the theme gives it some, or else these.
    pub fn chrome(&'static self) -> &'static Colours {
        self.chrome.unwrap_or(self)
    }

    /// egui's widgets draw with its visuals, which this gives in the theme's colours.
    pub fn visuals(&self) -> egui::Visuals {
        let mut visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        visuals.panel_fill = self.surface;
        visuals.window_fill = self.surface;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        visuals.faint_bg_color = self.surface_raised;
        visuals.extreme_bg_color = self.surface_sunken;
        visuals.code_bg_color = self.surface_sunken;
        visuals.hyperlink_color = self.accent;
        visuals.warn_fg_color = self.warning;
        visuals.error_fg_color = self.error;
        visuals.weak_text_color = Some(self.text_secondary);
        visuals.selection.bg_fill = self.accent;
        visuals.selection.stroke = Stroke::new(1.0, self.on_accent);
        visuals.window_shadow = Shadow { color: self.shadow, ..visuals.window_shadow };
        visuals.popup_shadow = Shadow { color: self.shadow, ..visuals.popup_shadow };

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.surface;
        widgets.noninteractive.weak_bg_fill = self.surface;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.divider);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text_primary);
        for (widget, fill) in [
            (&mut widgets.inactive, self.control),
            (&mut widgets.hovered, self.control_hovered),
            (&mut widgets.active, self.control_pressed),
            (&mut widgets.open, self.control_pressed),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.fg_stroke = Stroke::new(widget.fg_stroke.width, self.text_primary);
        }
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.hovered.bg_stroke = Stroke::new(1.0, self.border);
        widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        visuals
    }
}
