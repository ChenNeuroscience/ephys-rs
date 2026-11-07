//! Triggers that go straight to a node, on the GUI's thread, without needing a pass, or even a run: from a thread that's
//! outlived the run that started it, say.
//!
//! A trigger's sent to a node, through a settings edge or by the node itself, and which way it's delivered is the
//! difference between the two kinds. A pass trigger waits in the run's exchange for a pass to take it (see
//! `crate::core::exchange`): "auto detect" needs the data going by. A direct trigger goes to the app's `DirectTriggers`,
//! which it delivers as soon as it can, whether anything's running or not.
//!
//! Every setting takes one direct trigger without saying so: a new value for it (`Pass::trigger_settings_update`), which
//! the app applies as an edit, unless settings are locked. The rest are named (`Node::direct_triggers`), carry whatever
//! their sender gives them, and go to `Node::on_trigger`. A node sends one through a control with a `TriggerHandle`, from
//! `ControlTriggers`, which it can keep for later and use from any thread.

use std::any::Any;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;

use super::engine::NodeId;

/// What a direct trigger's for.
pub enum Triggered {
    /// A new value for the node's setting `setting` (by index).
    SettingsUpdate { setting: usize, value: Box<dyn Any + Send> },
    /// The node's trigger `name`, with what its sender gave it.
    Named { name: &'static str, payload: Box<dyn Any + Send> },
}

/// The direct triggers waiting to be delivered: the app's, so they outlive any run.
#[derive(Default)]
pub struct DirectTriggers {
    waiting: Mutex<Vec<(NodeId, Triggered)>>,
    /// Told when there's one, as a stopped app might not be drawing anything.
    wake: OnceLock<egui::Context>,
}

impl DirectTriggers {
    /// Has `ctx` draw a frame when a trigger comes, for it to be delivered.
    pub fn wake(&self, ctx: &egui::Context) {
        self.wake.get_or_init(|| ctx.clone());
    }

    fn send(&self, node: NodeId, triggered: Triggered) {
        self.waiting.lock().unwrap().push((node, triggered));
        if let Some(ctx) = self.wake.get() {
            ctx.request_repaint();
        }
    }

    /// Triggers an update of node `node`'s setting `setting` (by index) to `value`.
    pub fn trigger_settings_update(&self, node: NodeId, setting: usize, value: Box<dyn Any + Send>) {
        self.send(node, Triggered::SettingsUpdate { setting, value });
    }

    /// What's waiting, to deliver.
    pub fn take(&self) -> Vec<(NodeId, Triggered)> {
        std::mem::take(&mut *self.waiting.lock().unwrap())
    }

    /// The settings updates waiting: the node, which of its settings, and the value.
    #[cfg(test)]
    pub fn take_settings_updates(&self) -> Vec<(NodeId, usize, Box<dyn Any + Send>)> {
        let updates = self.take().into_iter().filter_map(|(node, triggered)| match triggered {
            Triggered::SettingsUpdate { setting, value } => Some((node, setting, value)),
            Triggered::Named { .. } => None,
        });
        updates.collect()
    }
}

impl Drop for DirectTriggers {
    /// Whatever's left never got to its node: the app closed first, while a thread finished up.
    fn drop(&mut self) {
        for (node, triggered) in self.take() {
            if let Triggered::Named { name, .. } = triggered {
                log::warn!("Node {}'s \"{name}\" was triggered as the app closed, and didn't happen", node.0);
            }
        }
    }
}

/// A named direct trigger of one or more nodes, to use whenever, from any thread (see `ControlTriggers`).
pub struct TriggerHandle {
    triggers: Arc<DirectTriggers>,
    nodes: Vec<NodeId>,
    name: &'static str,
}

impl TriggerHandle {
    /// Triggers it, with `payload` (cloned for each node).
    pub fn trigger<T: Clone + Send + 'static>(&self, payload: T) {
        for node in &self.nodes {
            let payload = Box::new(payload.clone());
            self.triggers.send(*node, Triggered::Named { name: self.name, payload });
        }
    }
}

/// What a node's controls can trigger directly: for each control, the nodes it connects to, and the direct triggers of
/// the setting of theirs it connects to.
pub struct ControlTriggers {
    triggers: Arc<DirectTriggers>,
    controls: Vec<Vec<(NodeId, &'static [&'static str])>>,
}

impl ControlTriggers {
    pub fn new(triggers: Arc<DirectTriggers>, controls: Vec<Vec<(NodeId, &'static [&'static str])>>) -> Self {
        ControlTriggers { triggers, controls }
    }

    /// None to trigger.
    pub fn none() -> Self {
        ControlTriggers { triggers: Arc::default(), controls: Vec::new() }
    }

    /// A handle on the direct trigger `name` of what control `control` connects to, if that has one.
    pub fn handle(&self, control: usize, name: &'static str) -> Option<TriggerHandle> {
        let targets = self.controls.get(control)?.iter();
        let nodes: Vec<NodeId> = targets.filter(|(_, names)| names.contains(&name)).map(|(node, _)| *node).collect();
        (!nodes.is_empty()).then(|| TriggerHandle { triggers: self.triggers.clone(), nodes, name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_triggers_what_it_connects_to_that_has_the_trigger() {
        let triggers = Arc::new(DirectTriggers::default());
        let both: &'static [&'static str] = &["go", "stop"];
        let neither: &'static [&'static str] = &[];
        let controls = vec![vec![], vec![(NodeId(1), both), (NodeId(2), neither), (NodeId(3), both)]];
        let controls = ControlTriggers::new(triggers.clone(), controls);
        assert!(controls.handle(0, "go").is_none(), "connected to nothing");
        assert!(controls.handle(1, "other").is_none(), "nothing has it");
        assert!(controls.handle(2, "go").is_none(), "no such control");
        controls.handle(1, "go").unwrap().trigger(7_u32);
        let delivered: Vec<(NodeId, u32)> = triggers
            .take()
            .into_iter()
            .map(|(node, triggered)| match triggered {
                Triggered::Named { name: "go", payload } => (node, *payload.downcast::<u32>().unwrap()),
                _ => panic!(),
            })
            .collect();
        assert_eq!(delivered, vec![(NodeId(1), 7), (NodeId(3), 7)]);
    }
}
