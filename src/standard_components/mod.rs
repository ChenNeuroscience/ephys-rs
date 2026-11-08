//! The standard components, grouped by what they're for: electrophysiology (`ephys`), spikes (`spikes`) and
//! video tracking (`tracking`), each with its edge types and node types; alarms on them all (`alarms`); writers, which
//! record them (`recording`); programs to run on what's recorded (`other`); annotation types (`annotations`); and themes (`themes`). A program declares which of them it's built with; the
//! edge enums they use (`crate::DataStream`, `crate::Setting`...) are in lib.rs.

pub mod alarms;

/// The formats of what the standard components save: their node types' settings, and the base directories' (see
/// `crate::core::formats`).
pub const FORMATS: crate::core::formats::Layer =
    crate::core::formats::Layer { name: "std", oldest: 0, steps: &[formats::example] };

/// The standard components' steps from each format of what they save to the next (see `crate::core::formats`).
mod formats {
    use serde_json::Value;

    use crate::core::formats::SavedPart;

    /// Format 0 to 1: nothing changed. An example, of the shape a step takes: it matches what it's about, and changes
    /// that as JSON, from what was saved before to what's saved now. Renaming a setting of the Spike Detector, say:
    ///
    /// ```ignore
    /// if what == SavedPart::Node("SpikeDetector")
    ///     && let Some(node) = value.as_object_mut()
    ///     && let Some(old) = node.remove("threshold")
    /// {
    ///     node.insert("thresholds".into(), old);
    /// }
    /// ```
    pub fn example(what: SavedPart, value: &mut Value) {
        let _ = (what, value);
    }

    #[cfg(test)]
    mod tests {
        use serde_json::json;

        use crate::core::formats::{Formats, Layers};

        /// Something saved before formats is brought up to date through the example, unchanged.
        #[test]
        fn the_example_step_changes_nothing() {
            static STD: [crate::core::formats::Layer; 1] = [super::super::FORMATS];
            let layers = Layers::new(&STD);
            assert_eq!(layers.current()["std"], 1);
            let saved =
                json!({ "graph": { "nodes": { "1": { "node": { "type": "SpikeDetector", "threshold": 4 } } } } });
            let mut value = saved.clone();
            layers.upgrade(&Formats::new(), super::SavedPart::Project, &mut value).unwrap();
            assert_eq!(value, saved);
        }
    }
}
pub mod annotations;
pub mod auto_detect;
pub mod base_directories;
pub mod ephys;
pub mod logs;
pub mod other;
pub mod pool;
mod preview;
pub mod recording;
pub mod spikes;
pub mod themes;
pub mod tracking;
mod widgets;

/// The most samples (or frames) of a data stream at `rate` that fall in `duration` seconds.
fn per_pass(rate: f32, duration: f64) -> usize {
    (rate as f64 * duration).ceil() as usize + 1
}
