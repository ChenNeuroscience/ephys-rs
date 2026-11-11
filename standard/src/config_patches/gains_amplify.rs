//! Gains amplify, from 0.232.0: a Spike Gain's or Continuous Gain's gains used to take µV to whatever units, those of
//! ±`output_range` (128, Write LTX's int8 values, unless it was set), so a gain of 0.512 fit ±250 µV, as did the
//! default for a channel without one of its own. Now they multiply µV as an amplifier does, into amplified µV, which
//! Write LTX records over ±250 µV (its scales), a channel without one of its own being ×1; auto detect fills
//! ±`fill_uv`. So each gain is made what fits the same range as it did (0.512 is ×1), and auto detect fills ±250 µV:
//! what's recorded is the same.

use ephys_rs::config_patching::IncomingConfig;
use serde_json::{Value, json};

/// What a channel without a gain of its own fit, and what Write LTX records (×1 fitting it).
const FULL_SCALE_UV: f64 = 250.0;
/// The output range, unless it was set.
const OUTPUT_RANGE: f64 = 128.0;

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("std", "0.232.0") {
        return;
    }
    let gain_nodes = config.nodes().filter(|node| node["type"] == "SpikeGain" || node["type"] == "ContinuousGain");
    for gains in gain_nodes.filter_map(|node| node.pointer_mut("/settings/gains").and_then(Value::as_object_mut)) {
        if gains.contains_key("fill_uv") {
            continue;
        }
        let range = gains.remove("output_range").and_then(|r| r.as_f64()).filter(|r| *r > 0.0);
        let range = range.unwrap_or(OUTPUT_RANGE);
        if let Some(values) = gains.get_mut("gains").and_then(Value::as_array_mut) {
            for gain in values.iter_mut() {
                if let Some(g) = gain.as_f64() {
                    *gain = json!(g * FULL_SCALE_UV / range);
                }
            }
        }
        gains.insert("fill_uv".into(), json!(FULL_SCALE_UV));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::standard::spikes::Gains;
    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn patched(versions: Versions, kind: &str, gains: serde_json::Value) -> serde_json::Value {
        let node = json!({ "type": kind, "settings": { "gains": gains } });
        let mut config = IncomingConfig { versions, json: json!({ "graph": { "nodes": { "1": { "node": node } } } }) };
        super::patch(&mut config);
        config.json["graph"]["nodes"]["1"]["node"]["settings"]["gains"].clone()
    }

    fn read(kind: &str, gains: serde_json::Value) -> Gains {
        serde_json::from_value(patched(Versions::new(), kind, gains)).unwrap()
    }

    /// A gain of 0.512, over ±128, fit ±250 µV: it amplifies by 1, over ±250 µV, which fits the same.
    #[test]
    fn gains_saved_as_they_were_record_the_same() {
        let old = json!({ "gains": [0.512, -1.024], "output_range": 128.0, "scope": "Channel" });
        for kind in ["SpikeGain", "ContinuousGain"] {
            let gains = read(kind, old.clone());
            assert_eq!(gains.fill_uv, 250.0, "{kind}");
            assert!((gains.get(0) - 1.0).abs() < 1e-4, "{kind}: {}", gains.get(0));
            assert!((gains.range_uv(0) - 250.0).abs() < 1e-3 && (gains.range_uv(1) - 125.0).abs() < 1e-3, "{kind}");
            assert!(gains.inverted(1), "{kind}: upside down, still");
        }
        // An output range of its own: 1 over ±32768 fit ±32768 µV, which ×1 now fits over ±250, at 1/131.
        let int16 = read("SpikeGain", json!({ "gains": [1.0], "output_range": 32768.0 }));
        assert!((int16.range_uv(0) - 32768.0).abs() < 0.1, "{}", int16.range_uv(0));
    }

    /// A channel without a gain of its own fit ±250 µV, as it still does: the EEG's too, though new ones begin at ±1 mV.
    /// Without an output range, it was 128.
    #[test]
    fn channels_without_gains_of_their_own_record_the_same() {
        for kind in ["SpikeGain", "ContinuousGain"] {
            let gains = read(kind, json!({ "gains": [], "output_range": 128.0, "scope": "All" }));
            assert!((gains.range_uv(3) - 250.0).abs() < 1e-3, "{kind}: {}", gains.range_uv(3));
            let without = read(kind, json!({ "gains": [0.256] }));
            assert!((without.range_uv(0) - 500.0).abs() < 1e-3, "{kind}: {}", without.range_uv(0));
        }
    }

    /// Written since, as it is.
    #[test]
    fn gains_written_since_are_left_as_they_are() {
        let new = json!({ "gains": [1.0], "fill_uv": 1000.0, "scope": "Channel" });
        let versions = Versions::from([("std".to_string(), "0.232.0".to_string())]);
        assert_eq!(patched(versions, "ContinuousGain", new.clone()), new);
        assert_eq!(patched(Versions::new(), "ContinuousGain", new.clone()), new, "and so's what's had it done");
    }
}
