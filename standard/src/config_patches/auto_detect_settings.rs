//! Auto detect's settings, from 0.263.0: how long auto detect samples for was a setting of the Continuous Gain's, Spike
//! Gain's and Spike Detector's own (`sampling`). It's now in the setting auto detect chooses, grouped with when it was
//! started, as `detect.seconds`, so whatever sees the setting (a viewer, through a control) sees it too. The Spike
//! Detector's thresholds were just their values, a list; they're now `values`, beside `detect`.

use ephys_rs::config_patching::IncomingConfig;
use serde_json::{Value, json};

pub fn patch(config: &mut IncomingConfig) {
    if !config.written_before("std", "0.263.0") {
        return;
    }
    for node in config.nodes() {
        let setting = match node["type"].as_str() {
            Some("SpikeGain" | "ContinuousGain") => "gains",
            Some("SpikeDetector") => "thresholds",
            _ => continue,
        };
        let Some(settings) = node.get_mut("settings").and_then(Value::as_object_mut) else { continue };
        let seconds = settings.remove("sampling");
        let Some(value) = settings.get_mut(setting) else { continue };
        if value.is_array() {
            *value = json!({ "values": value.take() });
        }
        if let (Some(seconds), Some(value)) = (seconds, value.as_object_mut()) {
            value.insert("detect".into(), json!({ "seconds": seconds }));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::standard::ephys::continuous_gain::ContinuousGain;
    use crate::standard::spikes::spike_detector::SpikeDetector;
    use crate::standard::spikes::spike_gain::SpikeGain;
    use ephys_rs::config_patching::{IncomingConfig, Versions};

    fn patched(versions: Versions, kind: &str, settings: Value) -> Value {
        let node = json!({ "type": kind, "settings": settings });
        let mut config = IncomingConfig { versions, json: json!({ "graph": { "nodes": { "1": { "node": node } } } }) };
        super::patch(&mut config);
        config.json["graph"]["nodes"]["1"]["node"]["settings"].clone()
    }

    /// How long each sampled for comes with it, into its gains or thresholds; and the thresholds keep their values.
    #[test]
    fn how_long_auto_detect_samples_for_moves_into_its_setting() {
        let detector =
            patched(Versions::new(), "SpikeDetector", json!({ "thresholds": [50.0, 60.0], "sampling": 12.0 }));
        let detector: SpikeDetector = serde_json::from_value(detector).unwrap();
        assert_eq!(detector.thresholds.values, [50.0, 60.0]);
        assert_eq!(detector.thresholds.detect.seconds, 12.0);
        let gain = patched(Versions::new(), "SpikeGain", json!({ "gains": { "gains": [2.0] }, "sampling": 7.0 }));
        let gain: SpikeGain = serde_json::from_value(gain).unwrap();
        assert_eq!((gain.gains.gains.as_slice(), gain.gains.detect.seconds), ([2.0].as_slice(), 7.0));
        let gain = patched(Versions::new(), "ContinuousGain", json!({ "gains": { "scope": "All" }, "sampling": 30.0 }));
        let gain: ContinuousGain = serde_json::from_value(gain).unwrap();
        assert_eq!(gain.gains.detect.seconds, 30.0);
    }

    /// Written since, as it is.
    #[test]
    fn settings_written_since_are_left_as_they_are() {
        let new = json!({ "thresholds": { "values": [50.0], "detect": { "seconds": 9.0 } } });
        let versions = Versions::from([("std".to_string(), "0.263.0".to_string())]);
        assert_eq!(patched(versions, "SpikeDetector", new.clone()), new);
        assert_eq!(patched(Versions::new(), "SpikeDetector", new.clone()), new, "and so's what's had it done");
    }
}
