//! Alarms: whether what's coming in looks wrong, over a period chosen for each. They're added one at a time, from a
//! library of kinds (`AlarmKind`), each with its own settings. The Alarm Processor (`processor`) watches spikes, LFP,
//! positions and TTLs where the data is, with the alarms' settings in its window (`settings`); the Alarm Viewer (`viewer`)
//! shows how they stand, with switches for them, and can pip as they go off (`tone`). Between them go only those settings,
//! along a settings edge, and a few numbers for each alarm a pass, as `AlarmStates`.

pub mod processor;
mod settings;
mod tone;
pub mod viewer;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::core::engine::{DataStreamType, EdgeInfo, EdgeType};

/// What an alarm watches: each is an input of the Alarm Processor, in this order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    #[default]
    Spikes,
    Lfp,
    Positions,
    Ttl,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Spikes, Source::Lfp, Source::Positions, Source::Ttl];

    /// What it's called, where an alarm's said to be going off.
    pub fn name(self) -> &'static str {
        match self {
            Source::Spikes => "Spikes",
            Source::Lfp => "LFP",
            Source::Positions => "Positions",
            Source::Ttl => "TTLs",
        }
    }

    /// Which input of the Alarm Processor it is.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Which groups, channels or lines an alarm watches, and how.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    /// Which to leave out (from 0): there may be more of them later, so it's those left out that are kept.
    pub left_out: Vec<u32>,
    /// Whether it measures those selected together (their total, or for fractions, their mean), rather than each.
    pub together: bool,
}

impl Selection {
    pub fn selected(&self, item: u32) -> bool {
        !self.left_out.contains(&item)
    }
}

/// The kinds of alarm there are, each with the settings it needs. A new kind goes here and in `library`, and each `match`
/// on this says what it needs saying: there can be several for one `Source`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum AlarmKind {
    /// The average spike rate is outside `low` to `high`, in Hz.
    SpikeRate { low: f32, high: f32, selection: Selection },
    /// Fewer than `low` percent of LFP samples are within ±`level_uv`.
    LfpSaturation { level_uv: f32, low: f32, selection: Selection },
    /// Fewer than `low` percent of position samples have data.
    MissingPositions { low: f32 },
    /// The number of TTLs is outside `low` to `high`.
    TtlCount { low: f32, high: f32, selection: Selection },
    /// A TTL line (from 0) is up: straight away, with no period.
    TtlUp { line: u32 },
}

impl AlarmKind {
    /// What there is to add, as each starts out.
    pub fn library() -> [AlarmKind; 5] {
        [
            AlarmKind::SpikeRate {
                low: 5.0,
                high: 400.0,
                selection: Selection { together: true, ..Default::default() },
            },
            AlarmKind::LfpSaturation { level_uv: DEFAULT_SATURATION_UV, low: 95.0, selection: Selection::default() },
            AlarmKind::MissingPositions { low: 80.0 },
            AlarmKind::TtlCount { low: 1.0, high: 3.0, selection: Selection { together: true, ..Default::default() } },
            AlarmKind::TtlUp { line: 0 },
        ]
    }

    pub fn source(&self) -> Source {
        match self {
            AlarmKind::SpikeRate { .. } => Source::Spikes,
            AlarmKind::LfpSaturation { .. } => Source::Lfp,
            AlarmKind::MissingPositions { .. } => Source::Positions,
            AlarmKind::TtlCount { .. } | AlarmKind::TtlUp { .. } => Source::Ttl,
        }
    }

    /// Its name, which says what's healthy.
    pub fn name(&self) -> &'static str {
        match self {
            AlarmKind::SpikeRate { .. } => "Spike rate in range",
            AlarmKind::LfpSaturation { .. } => "LFP within range",
            AlarmKind::MissingPositions { .. } => "Position data available",
            AlarmKind::TtlCount { .. } => "TTL count in range",
            AlarmKind::TtlUp { .. } => "TTL line down",
        }
    }

    /// What it's for, in the list of what can be added.
    pub fn description(&self) -> &'static str {
        match self {
            AlarmKind::SpikeRate { .. } => "When the average spike rate is outside a range",
            AlarmKind::LfpSaturation { .. } => "When too much of the LFP is beyond a level",
            AlarmKind::MissingPositions { .. } => "When too few position samples have data",
            AlarmKind::TtlCount { .. } => "When the number of TTLs is outside a range",
            AlarmKind::TtlUp { .. } => "For as long as a TTL line is up",
        }
    }

    /// The period it starts with, in seconds.
    fn seconds(&self) -> f32 {
        match self {
            AlarmKind::TtlCount { .. } => 30.0,
            _ => DEFAULT_SECONDS,
        }
    }

    /// Whether it's evaluated over a period, rather than going by what's happening now.
    pub fn has_period(&self) -> bool {
        !matches!(self, AlarmKind::TtlUp { .. })
    }

    /// What it measures has to stay within this.
    pub fn range(&self) -> (f32, f32) {
        match *self {
            AlarmKind::SpikeRate { low, high, .. } | AlarmKind::TtlCount { low, high, .. } => (low, high),
            AlarmKind::LfpSaturation { low, .. } | AlarmKind::MissingPositions { low } => (low, 100.0),
            AlarmKind::TtlUp { .. } => (0.0, 0.0),
        }
    }

    /// Which items it watches, if it can choose: otherwise it's all of them, each on its own.
    pub fn selection(&self) -> Option<&Selection> {
        match self {
            AlarmKind::SpikeRate { selection, .. }
            | AlarmKind::LfpSaturation { selection, .. }
            | AlarmKind::TtlCount { selection, .. } => Some(selection),
            AlarmKind::MissingPositions { .. } | AlarmKind::TtlUp { .. } => None,
        }
    }

    pub fn selection_mut(&mut self) -> Option<&mut Selection> {
        match self {
            AlarmKind::SpikeRate { selection, .. }
            | AlarmKind::LfpSaturation { selection, .. }
            | AlarmKind::TtlCount { selection, .. } => Some(selection),
            AlarmKind::MissingPositions { .. } | AlarmKind::TtlUp { .. } => None,
        }
    }

    /// A value as it's shown: "12.5 Hz", "99.8% in range".
    pub fn value(&self, value: f32) -> String {
        match self {
            AlarmKind::SpikeRate { .. } => format!("{value:.1} Hz"),
            AlarmKind::LfpSaturation { .. } => format!("{value:.1}% in range"),
            AlarmKind::MissingPositions { .. } => format!("{value:.1}% with data"),
            AlarmKind::TtlCount { .. } if value == 1.0 => "1 TTL".to_owned(),
            AlarmKind::TtlCount { .. } => format!("{value:.0} TTLs"),
            AlarmKind::TtlUp { .. } => if value > 0.0 { "Line up" } else { "Line down" }.to_owned(),
        }
    }
}

/// An alarm that's been added: it goes off when what it measures, over the last `seconds`, is outside its kind's range.
/// One that's off is kept, for coming back to, but not watched.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Alarm {
    /// Which alarm this is, whatever else is added and removed.
    pub id: u32,
    pub on: bool,
    pub seconds: f32,
    pub kind: AlarmKind,
}

/// The periods an alarm can be over, in seconds.
pub const PERIODS: std::ops::RangeInclusive<f32> = 1.0..=3600.0;
const DEFAULT_SECONDS: f32 = 20.0;
/// How far from zero an LFP sample counts as saturated until it's set, in µV: a headstage's rails are at ±6389 µV,
/// though filtering takes the peaks of what reaches them down a little.
const DEFAULT_SATURATION_UV: f32 = 6300.0;

impl Alarm {
    pub fn period(&self) -> f64 {
        self.seconds.clamp(*PERIODS.start(), *PERIODS.end()) as f64
    }
}

/// All the alarms: the Alarm Processor's settings, which the Alarm Viewer edits.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Alarms {
    pub list: Vec<Alarm>,
    /// The next alarm's id: they aren't reused, so nothing's taken for one that's gone.
    next_id: u32,
}

impl Alarms {
    /// Adds an alarm of `kind`, on.
    pub fn add(&mut self, kind: AlarmKind) -> &mut Alarm {
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Alarm { id, on: true, seconds: kind.seconds(), kind });
        let last = self.list.len() - 1;
        &mut self.list[last]
    }

    pub fn remove(&mut self, id: u32) {
        self.list.retain(|alarm| alarm.id != id);
    }

    pub fn get(&self, id: u32) -> Option<&Alarm> {
        self.list.iter().find(|alarm| alarm.id == id)
    }

    /// Whether there are alarms, and they're all on.
    pub fn all_on(&self) -> bool {
        !self.list.is_empty() && self.list.iter().all(|alarm| alarm.on)
    }

    /// Whether any alarm that's on watches `source`.
    pub fn watches(&self, source: Source) -> bool {
        self.list.iter().any(|alarm| alarm.on && alarm.kind.source() == source)
    }
}

impl EdgeType for Alarms {
    const INFO: EdgeInfo = EdgeInfo {
        name: "alarms",
        colour: Color32::from_rgb(235, 90, 80),
        description: "What each alarm watches, and when it goes off.",
    };

    fn summary(&self) -> String {
        let on = self.list.iter().filter(|alarm| alarm.on).count();
        format!("{on} on")
    }
}

/// What the alarms have to choose from: how they stand comes with each pass (see `AlarmStatesBuffer`), so adding one doesn't
/// change this. Counts are 0 for what's not connected.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct AlarmStates {
    pub groups: u32,
    /// Channels in each group of spikes.
    pub group_size: u32,
    pub channels: u32,
    pub positions: bool,
    pub lines: u32,
}

impl AlarmStates {
    /// How many groups, channels or lines `source` has, which are chosen from: 0 if it's not connected.
    pub fn items(&self, source: Source) -> u32 {
        match source {
            Source::Spikes => self.groups,
            Source::Lfp => self.channels,
            Source::Positions => self.positions as u32,
            Source::Ttl => self.lines,
        }
    }

    /// What `source`'s items are called: "tetrode", say. (Positions have just the one.)
    pub fn item_name(&self, source: Source) -> &'static str {
        match (source, self.group_size) {
            (Source::Spikes, 1) => "channel",
            (Source::Spikes, 2) => "stereotrode",
            (Source::Spikes, 4) => "tetrode",
            (Source::Spikes, _) => "group",
            (Source::Lfp, _) => "channel",
            (Source::Positions, _) => "camera",
            (Source::Ttl, _) => "line",
        }
    }
}

impl EdgeType for AlarmStates {
    const INFO: EdgeInfo = EdgeInfo {
        name: "alarm states",
        colour: Color32::from_rgb(235, 90, 80),
        description: "How each alarm stands.",
    };

    fn summary(&self) -> String {
        let watched = Source::ALL.iter().filter(|s| self.items(**s) > 0).count();
        format!("{watched} watched")
    }
}

/// Where an alarm's got to.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Phase {
    /// Off, or with nothing to watch.
    #[default]
    Off,
    /// Yet to see a whole period, this fraction of the way there.
    Waiting(f32),
    Clear,
    Alarm,
}

/// How an alarm stands.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct AlarmState {
    /// Which alarm (see `Alarm::id`).
    pub id: u32,
    pub source: Source,
    pub phase: Phase,
    /// What it measured over its period.
    pub value: f32,
    /// The group, channel or line that's measured, if it's one of them: the furthest from the range.
    pub item: Option<u32>,
}

/// A pass's states, one for each alarm.
#[derive(Default)]
pub struct AlarmStatesBuffer {
    pub states: Vec<AlarmState>,
}

impl DataStreamType for AlarmStates {
    type Buffer = AlarmStatesBuffer;

    fn buffer(&self, _duration: f64) -> AlarmStatesBuffer {
        AlarmStatesBuffer::default()
    }

    fn bytes(buffer: &AlarmStatesBuffer) -> usize {
        size_of_val(buffer.states.as_slice())
    }

    fn copy(from: &AlarmStatesBuffer, to: &mut AlarmStatesBuffer) {
        to.states.clone_from(&from.states);
    }
}
