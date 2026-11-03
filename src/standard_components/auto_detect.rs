//! Choosing settings from the data ("auto detect"): a node's trigger (see `Node::triggers`), which its passes take,
//! sample the data for a while, then propose settings from. The Spike Gain, Continuous Gain and Spike Detector have one,
//! fired from their windows or a viewer's; these are the pieces they share.

use eframe::egui;

use crate::Edges;
use crate::core::engine::Context;

/// The trigger.
pub const AUTO_DETECT: &str = "auto detect";
/// How long the passes can sample for, in seconds.
pub const SAMPLING: std::ops::RangeInclusive<f32> = 4.0..=60.0;

/// A button to fire the auto-detect trigger (of the node's own settings, or of the setting `control` connects to),
/// and how it's going: for this node's window, and a viewer's.
pub fn auto_detect_ui(ui: &mut egui::Ui, context: &mut Context<Edges>, control: Option<usize>, tip: &str) {
    ui.horizontal(|ui| {
        // While detecting, it can be stopped.
        if let Some(progress) = context.trigger_progress(control, AUTO_DETECT) {
            if ui.button("Stop").on_hover_text("Stop detecting, leaving things as they were").clicked() {
                context.cancel_trigger(control, AUTO_DETECT);
            }
            ui.add(egui::ProgressBar::new(progress).desired_width(80.0).show_percentage());
            return;
        }
        let can = context.can_trigger(control, AUTO_DETECT) && !context.locked;
        let why = if context.locked { "Not while recording" } else { "Only while acquiring: it needs data" };
        if ui
            .add_enabled(can, egui::Button::new("Auto detect"))
            .on_hover_text(tip)
            .on_disabled_hover_text(why)
            .clicked()
        {
            context.trigger(control, AUTO_DETECT);
        }
    });
}

/// Whether the node's auto-detect is going (its settings for it can't change meanwhile).
pub fn detecting(context: &Context<Edges>) -> bool {
    context.trigger_progress(None, AUTO_DETECT).is_some()
}
