//! Triggers: something a node's sent, through a settings edge or by the node itself, as well as its settings being set.
//!
//! Each trigger type (`TriggerType`) carries what it needs to, and the program lists them with its edge types
//! (`edge_types!`), which makes its enum of them (`TriggersEnum`), as it does of data streams and settings. A node says
//! which kinds of them each of its settings takes (`Node::triggers`). How one's delivered is up to its type
//! (`TriggerType::DELIVERY`), and sending one is the same either way (`UiContext::trigger`). A pass trigger waits in the
//! run's `PassTriggers` for a pass to take it (`Pass::triggered`; see `crate::pass_triggers`): "auto detect" needs the data
//! going by, so it needs a run, and says how it's getting on. A direct trigger goes to the app's `DirectTriggers`, which
//! it delivers to `Node::on_trigger` on the GUI's thread as soon as it can, whether anything's running or not: from a
//! thread that's outlived the run that started it, say. A node sends one through a control with a `TriggerHandle`, from
//! `ControlTriggers`, which it can keep for later and use from any thread.
//!
//! Every setting also takes a direct update of its value, without saying so (`Pass::trigger_settings_update`), which
//! the app applies as an edit, unless settings are locked.
//!
//! On their way, triggers are carried as `AnyTrigger`s, so the run's and the app's queues needn't know the program's
//! enum; what's sent and what's taken is always the enum.

use std::any::Any;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;

use super::engine::NodeId;

/// A type of trigger: something a node can be sent, through a settings edge or by itself, carrying whatever it needs to
/// (`RunOnFiles` its files, say). The program lists them in `edge_types!`.
pub trait TriggerType: Send + 'static {
    /// What it's called where it's shown: in the dev tools, say.
    const NAME: &'static str;
    /// How it gets to the node.
    const DELIVERY: Delivery;
}

/// How a trigger gets to the node it's sent to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Delivery {
    /// To a pass, which takes it (`Pass::triggered`) with the data going by, says how it's getting on, and when it's
    /// done. Only while running.
    Pass,
    /// Straight to the node, on the GUI's thread (`Node::on_trigger`), running or not.
    Direct,
}

/// The program's trigger types: an enum with a variant for each, made by `edge_types!`.
pub trait TriggersEnum: Clone + Send + 'static {
    /// Which type a trigger is, without what it carries: what a node says its settings take.
    type Kind: Copy + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + Into<TriggerKey> + 'static;

    fn kind(&self) -> Self::Kind;
}

/// A kind of trigger, as the core keeps track of them without knowing the program's enum: which variant it is, what
/// it's called and how it's delivered. Each `TriggersEnum::Kind` converts into one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TriggerKey {
    pub index: usize,
    pub name: &'static str,
    pub delivery: Delivery,
}

/// A trigger on its way, of the program's `TriggersEnum`: taken as that by whatever it's for.
pub struct AnyTrigger {
    key: TriggerKey,
    trigger: Box<dyn Any + Send>,
}

impl AnyTrigger {
    pub fn new<T: TriggersEnum>(trigger: T) -> Self {
        AnyTrigger { key: trigger.kind().into(), trigger: Box::new(trigger) }
    }

    pub fn key(&self) -> TriggerKey {
        self.key
    }

    /// It, as the program's `T`, if that's what it is.
    pub fn take<T: TriggersEnum>(self) -> Option<T> {
        self.trigger.downcast().ok().map(|trigger| *trigger)
    }
}

/// What a direct trigger's for.
pub enum Triggered {
    /// A new value for the node's setting `setting` (by index).
    SettingsUpdate { setting: usize, value: Box<dyn Any + Send> },
    /// One of the node's direct triggers (see `Node::triggers`).
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

    /// Sends node `node` the direct trigger `trigger`.
    pub fn trigger(&self, node: NodeId, trigger: AnyTrigger) {
        debug_assert_eq!(trigger.key().delivery, Delivery::Direct, "{}", trigger.key().name);
        self.send(node, Triggered::Trigger(trigger));
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
                let name = trigger.key().name;
                log::warn!("Node {}'s \"{name}\" was triggered as the app closed, and didn't happen", node.0);
            }
        }
    }
}

/// A direct trigger of one or more nodes, of one kind of the program's `T`, to send whenever, from any thread (see
/// `ControlTriggers`).
pub struct TriggerHandle<T: TriggersEnum> {
    triggers: Arc<DirectTriggers>,
    nodes: Vec<NodeId>,
    kind: T::Kind,
}

impl<T: TriggersEnum> TriggerHandle<T> {
    /// Sends `trigger` (cloned for each node), which should be of the handle's kind.
    pub fn trigger(&self, trigger: impl Into<T>) {
        let trigger = trigger.into();
        debug_assert_eq!(trigger.kind(), self.kind, "the handle's kind of trigger");
        for node in &self.nodes {
            self.triggers.trigger(*node, AnyTrigger::new(trigger.clone()));
        }
    }
}

/// What a control connects to: each node, and the kinds of the program's triggers `T` its setting takes.
type Targets<T> = Vec<(NodeId, &'static [<T as TriggersEnum>::Kind])>;

/// What a node's controls can trigger directly: for each control, the nodes it connects to, and the kinds of the
/// program's triggers `T` the setting of theirs it connects to takes.
pub struct ControlTriggers<T: TriggersEnum> {
    triggers: Arc<DirectTriggers>,
    controls: Vec<Targets<T>>,
}

impl<T: TriggersEnum> ControlTriggers<T> {
    pub fn new(triggers: Arc<DirectTriggers>, controls: Vec<Targets<T>>) -> Self {
        ControlTriggers { triggers, controls }
    }

    /// None to trigger.
    pub fn none() -> Self {
        ControlTriggers { triggers: Arc::default(), controls: Vec::new() }
    }

    /// A handle on triggers of kind `kind` to what control `control` connects to, if that takes them. Only of a direct
    /// trigger: a handle can outlive the run, which a pass trigger needs.
    pub fn handle(&self, control: usize, kind: T::Kind) -> Option<TriggerHandle<T>> {
        if kind.into().delivery != Delivery::Direct {
            return None;
        }
        let targets = self.controls.get(control)?.iter();
        let nodes: Vec<NodeId> = targets.filter(|(_, kinds)| kinds.contains(&kind)).map(|(node, _)| *node).collect();
        (!nodes.is_empty()).then(|| TriggerHandle { triggers: self.triggers.clone(), nodes, kind })
    }
}

/// A program's triggers, for tests: `Go`, a direct trigger carrying a number, and `Level`, a pass trigger carrying a
/// level, as `edge_types!` would make them.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    pub enum TestTrigger {
        Go(u32),
        Level(f32),
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    pub enum TestKind {
        Go,
        Level,
    }

    impl From<TestKind> for TriggerKey {
        fn from(kind: TestKind) -> Self {
            match kind {
                TestKind::Go => TriggerKey { index: 0, name: "go", delivery: Delivery::Direct },
                TestKind::Level => TriggerKey { index: 1, name: "level", delivery: Delivery::Pass },
            }
        }
    }

    impl TriggersEnum for TestTrigger {
        type Kind = TestKind;

        fn kind(&self) -> TestKind {
            match self {
                TestTrigger::Go(_) => TestKind::Go,
                TestTrigger::Level(_) => TestKind::Level,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{TestKind, TestTrigger};
    use super::*;

    #[test]
    fn a_control_triggers_what_it_connects_to_that_takes_the_trigger() {
        let triggers = Arc::new(DirectTriggers::default());
        let both: &'static [TestKind] = &[TestKind::Go, TestKind::Level];
        let neither: &'static [TestKind] = &[];
        let controls = vec![vec![], vec![(NodeId(1), both), (NodeId(2), neither), (NodeId(3), both)]];
        let controls = ControlTriggers::<TestTrigger>::new(triggers.clone(), controls);
        assert!(controls.handle(0, TestKind::Go).is_none(), "connected to nothing");
        assert!(controls.handle(2, TestKind::Go).is_none(), "no such control");
        assert!(controls.handle(1, TestKind::Level).is_none(), "a pass trigger: a handle could outlive the run");
        controls.handle(1, TestKind::Go).unwrap().trigger(TestTrigger::Go(7));
        let delivered: Vec<(NodeId, TestTrigger)> = triggers
            .take()
            .into_iter()
            .map(|(node, triggered)| match triggered {
                Triggered::Trigger(trigger) => (node, trigger.take::<TestTrigger>().unwrap()),
                _ => panic!(),
            })
            .collect();
        assert_eq!(delivered, vec![(NodeId(1), TestTrigger::Go(7)), (NodeId(3), TestTrigger::Go(7))]);
    }

    /// What `edge_types!` makes: each kind's name and delivery are its type's.
    #[test]
    fn the_programs_trigger_kinds_say_their_types_name_and_delivery() {
        let key = |kind: crate::TriggerKind| TriggerKey::from(kind);
        assert_eq!(
            key(crate::TriggerKind::AutoDetect),
            TriggerKey { index: 0, name: "auto detect", delivery: Delivery::Pass }
        );
        let run = key(crate::TriggerKind::RunOnFiles);
        assert_eq!((run.index, run.name, run.delivery), (1, "run on files", Delivery::Direct));
        let trigger = crate::Trigger::from(crate::standard::other::run_command::RunOnFiles(Vec::new()));
        assert_eq!(trigger.kind(), crate::TriggerKind::RunOnFiles);
    }

    #[test]
    fn a_trigger_on_its_way_is_taken_as_what_it_is() {
        let trigger = AnyTrigger::new(TestTrigger::Go(3));
        assert_eq!(trigger.key(), TestKind::Go.into());
        assert_eq!(trigger.key().name, "go");
        assert_eq!(trigger.take::<TestTrigger>(), Some(TestTrigger::Go(3)));
    }
}
