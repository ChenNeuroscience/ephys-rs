//! Triggers that go straight to a node, on the GUI's thread, without needing a pass, or even a run: from a thread that's
//! outlived the run that started it, say.
//!
//! A trigger's sent to a node, through a settings edge or by the node itself, and which way it's delivered is the
//! difference between the two kinds. A pass trigger waits in the run's exchange for a pass to take it (see
//! `crate::core::exchange`): "auto detect" needs the data going by. A direct trigger goes to the app's `DirectTriggers`,
//! which it delivers as soon as it can, whether anything's running or not.
//!
//! Every setting takes one direct trigger without saying so: a new value for it (`Pass::trigger_settings_update`), which
//! the app applies as an edit, unless settings are locked. The rest are `Trigger`s, each a type of its own carrying what
//! it needs to, which a node says its settings take (`Node::direct_triggers`), and which go to `Node::on_trigger`. A node
//! sends one through a control with a `TriggerHandle`, from `ControlTriggers`, which it can keep for later and use from
//! any thread. Pass triggers are `Trigger`s too (see `Node::triggers`).

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;

use super::engine::NodeId;

/// Something a node can be sent, through a settings edge or by itself: a type of its own, carrying whatever it needs
/// to (`RunOnFiles` its files, say), so what's sent is what's taken, whichever way it's delivered.
pub trait Trigger: Sized + Send + 'static {
    /// What it's called where it's shown: in the dev tools, say.
    const NAME: &'static str;
    /// Its kind, for a node to list (`&[AutoDetect::KIND]`, say).
    const KIND: TriggerKind = TriggerKind::of::<Self>();
}

/// A kind of trigger, without what one carries: what a node says its settings take, and what a trigger's progress is
/// kept by.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TriggerKind {
    id: TypeId,
    pub name: &'static str,
}

impl TriggerKind {
    pub const fn of<T: Trigger>() -> Self {
        TriggerKind { id: TypeId::of::<T>(), name: T::NAME }
    }
}

/// A trigger of some kind, on its way: taken as what it is by whatever it's for.
pub struct AnyTrigger {
    kind: TriggerKind,
    trigger: Box<dyn Any + Send>,
}

impl AnyTrigger {
    pub fn new<T: Trigger>(trigger: T) -> Self {
        AnyTrigger { kind: TriggerKind::of::<T>(), trigger: Box::new(trigger) }
    }

    pub fn kind(&self) -> TriggerKind {
        self.kind
    }

    /// It, if it's a `T`; otherwise, itself back.
    pub fn take<T: Trigger>(self) -> Result<T, Self> {
        match self.trigger.downcast::<T>() {
            Ok(trigger) => Ok(*trigger),
            Err(trigger) => Err(AnyTrigger { kind: self.kind, trigger }),
        }
    }
}

/// What a direct trigger's for.
pub enum Triggered {
    /// A new value for the node's setting `setting` (by index).
    SettingsUpdate { setting: usize, value: Box<dyn Any + Send> },
    /// One of the node's direct triggers (see `Node::direct_triggers`).
    Trigger(AnyTrigger),
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
            Triggered::Trigger(_) => None,
        });
        updates.collect()
    }
}

impl Drop for DirectTriggers {
    /// Whatever's left never got to its node: the app closed first, while a thread finished up.
    fn drop(&mut self) {
        for (node, triggered) in self.take() {
            if let Triggered::Trigger(trigger) = triggered {
                let name = trigger.kind().name;
                log::warn!("Node {}'s \"{name}\" was triggered as the app closed, and didn't happen", node.0);
            }
        }
    }
}

/// A direct trigger of one or more nodes, a `T`, to send whenever, from any thread (see `ControlTriggers`).
pub struct TriggerHandle<T> {
    triggers: Arc<DirectTriggers>,
    nodes: Vec<NodeId>,
    kind: PhantomData<fn(T)>,
}

impl<T: Trigger + Clone> TriggerHandle<T> {
    /// Sends `trigger` (cloned for each node).
    pub fn trigger(&self, trigger: T) {
        for node in &self.nodes {
            self.triggers.send(*node, Triggered::Trigger(AnyTrigger::new(trigger.clone())));
        }
    }
}

/// What a node's controls can trigger directly: for each control, the nodes it connects to, and the direct triggers of
/// the setting of theirs it connects to.
pub struct ControlTriggers {
    triggers: Arc<DirectTriggers>,
    controls: Vec<Vec<(NodeId, &'static [TriggerKind])>>,
}

impl ControlTriggers {
    pub fn new(triggers: Arc<DirectTriggers>, controls: Vec<Vec<(NodeId, &'static [TriggerKind])>>) -> Self {
        ControlTriggers { triggers, controls }
    }

    /// None to trigger.
    pub fn none() -> Self {
        ControlTriggers { triggers: Arc::default(), controls: Vec::new() }
    }

    /// A handle on the direct trigger `T` of what control `control` connects to, if that takes one.
    pub fn handle<T: Trigger>(&self, control: usize) -> Option<TriggerHandle<T>> {
        let kind = TriggerKind::of::<T>();
        let targets = self.controls.get(control)?.iter();
        let nodes: Vec<NodeId> = targets.filter(|(_, kinds)| kinds.contains(&kind)).map(|(node, _)| *node).collect();
        (!nodes.is_empty()).then(|| TriggerHandle { triggers: self.triggers.clone(), nodes, kind: PhantomData })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Go(u32);
    impl Trigger for Go {
        const NAME: &'static str = "go";
    }
    struct Stop;
    impl Trigger for Stop {
        const NAME: &'static str = "stop";
    }

    #[test]
    fn a_control_triggers_what_it_connects_to_that_takes_the_trigger() {
        let triggers = Arc::new(DirectTriggers::default());
        let both: &'static [TriggerKind] = &[Go::KIND, Stop::KIND];
        let neither: &'static [TriggerKind] = &[];
        let controls = vec![vec![], vec![(NodeId(1), both), (NodeId(2), neither), (NodeId(3), both)]];
        let controls = ControlTriggers::new(triggers.clone(), controls);
        assert!(controls.handle::<Go>(0).is_none(), "connected to nothing");
        assert!(controls.handle::<Go>(2).is_none(), "no such control");
        controls.handle::<Go>(1).unwrap().trigger(Go(7));
        let delivered: Vec<(NodeId, Go)> = triggers
            .take()
            .into_iter()
            .map(|(node, triggered)| match triggered {
                Triggered::Trigger(trigger) => (node, trigger.take::<Go>().ok().unwrap()),
                _ => panic!(),
            })
            .collect();
        assert_eq!(delivered, vec![(NodeId(1), Go(7)), (NodeId(3), Go(7))]);
    }

    #[test]
    fn a_trigger_is_taken_as_what_it_is() {
        let trigger = AnyTrigger::new(Go(3));
        assert_eq!(trigger.kind(), TriggerKind::of::<Go>());
        assert_eq!(trigger.kind().name, "go");
        let trigger = trigger.take::<Stop>().err().expect("not a Stop");
        assert_eq!(trigger.take::<Go>().ok(), Some(Go(3)));
    }
}
