//! Alarms: whether what's coming in looks wrong, over a period chosen for each. The Alarm Detector (`detector`) watches
//! spikes, LFP, positions and TTLs where the data is; the Alarms viewer (`viewer`) shows how they stand. Between them
//! go only their settings, along a settings edge, and a few numbers a pass, as `AlarmStates`.

pub mod detector;
mod tone;
pub mod viewer;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::core::engine::{DataStreamType, EdgeInfo, EdgeType};

/// What an alarm watches: each is an input of the Alarm Detector, in this order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Spikes,
    Lfp,
    Positions,
    Ttl,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Spikes, Source::Lfp, Source::Positions, Source::Ttl];

    pub fn name(self) -> &'static str {
        match self {
            Source::Spikes => "Spikes",
            Source::Lfp => "LFP",
            Source::Positions => "Positions",
            Source::Ttl => "TTLs",
        }
    }

    /// What's measured, for the range: "Hz", say.
    pub fn unit(self) -> &'static str {
        match self {
            Source::Spikes => " Hz",
            Source::Lfp | Source::Positions => " %",
            Source::Ttl => "",
        }
    }

    /// A value as it's shown: "12.5 Hz", "3% saturated".
    pub fn value(self, value: f32) -> String {
        match self {
            Source::Spikes => format!("{value:.1} Hz"),
            Source::Lfp => format!("{value:.1}% saturated"),
            Source::Positions => format!("{value:.1}% no data"),
            Source::Ttl => format!("{value:.0} TTLs"),
        }
    }
}

/// One alarm's settings: it goes off when what it measures, over the last `seconds`, is outside `low` to `high`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Alarm {
    pub on: bool,
    pub seconds: f32,
    pub low: f32,
    pub high: f32,
    /// Which groups, channels or lines (from 0) to leave out: there may be more of them later, so it's those left out
    /// that are kept.
    pub left_out: Vec<u32>,
    /// Whether it measures those selected together (their total, or for fractions, their mean), rather than each.
    pub together: bool,
}

impl Default for Alarm {
    fn default() -> Self {
        Alarm { on: true, seconds: DEFAULT_SECONDS, low: 0.0, high: 100.0, left_out: Vec::new(), together: false }
    }
}

/// The periods an alarm can be over, in seconds.
pub const PERIODS: std::ops::RangeInclusive<f32> = 1.0..=3600.0;
const DEFAULT_SECONDS: f32 = 20.0;
/// How near the rails an LFP sample counts as saturated until it's set, in µV: a headstage's rails are at ±6389 µV.
const DEFAULT_SATURATION_UV: f32 = 6300.0;

impl Alarm {
    pub fn period(&self) -> f64 {
        self.seconds.clamp(*PERIODS.start(), *PERIODS.end()) as f64
    }

    pub fn selected(&self, item: u32) -> bool {
        !self.left_out.contains(&item)
    }
}

/// All the alarms' settings: the Alarm Detector's, which the Alarms viewer edits.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Alarms {
    /// Whether they're on at all.
    pub on: bool,
    pub spikes: Alarm,
    pub lfp: Alarm,
    pub positions: Alarm,
    pub ttl: Alarm,
    /// How far from zero an LFP sample counts as saturated, in µV.
    pub saturation_uv: f32,
}

impl Default for Alarms {
    fn default() -> Self {
        Alarms {
            on: true,
            spikes: Alarm { low: 5.0, high: 400.0, together: true, ..Default::default() },
            lfp: Alarm { high: 5.0, ..Default::default() },
            positions: Alarm { high: 20.0, ..Default::default() },
            ttl: Alarm { low: 1.0, high: 10_000.0, together: true, ..Default::default() },
            saturation_uv: DEFAULT_SATURATION_UV,
        }
    }
}

impl Alarms {
    pub fn alarm(&self, source: Source) -> &Alarm {
        match source {
            Source::Spikes => &self.spikes,
            Source::Lfp => &self.lfp,
            Source::Positions => &self.positions,
            Source::Ttl => &self.ttl,
        }
    }

    pub fn alarm_mut(&mut self, source: Source) -> &mut Alarm {
        match source {
            Source::Spikes => &mut self.spikes,
            Source::Lfp => &mut self.lfp,
            Source::Positions => &mut self.positions,
            Source::Ttl => &mut self.ttl,
        }
    }

    /// Whether `source`'s alarm is on, all told.
    pub fn watching(&self, source: Source) -> bool {
        self.on && self.alarm(source).on
    }
}

impl EdgeType for Alarms {
    const INFO: EdgeInfo = EdgeInfo {
        name: "alarms",
        colour: Color32::from_rgb(235, 90, 80),
        description: "What each alarm watches, and when it goes off.",
    };

    fn summary(&self) -> String {
        let on = Source::ALL.iter().filter(|s| self.watching(**s)).count();
        format!("{on} on")
    }
}

/// How the alarms stand: a few numbers a pass, and what there is to choose from. Counts are 0 for what's not connected.
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
    pub phase: Phase,
    /// What it measured over its period.
    pub value: f32,
    /// The group, channel or line that's measured, if it's one of them: the furthest from the range.
    pub item: Option<u32>,
}

/// A pass's states, by `Source`.
#[derive(Default)]
pub struct AlarmStatesBuffer {
    pub states: [AlarmState; 4],
}

impl DataStreamType for AlarmStates {
    type Buffer = AlarmStatesBuffer;

    fn buffer(&self, _duration: f64) -> AlarmStatesBuffer {
        AlarmStatesBuffer::default()
    }

    fn bytes(_buffer: &AlarmStatesBuffer) -> usize {
        size_of::<AlarmStatesBuffer>()
    }

    fn copy(from: &AlarmStatesBuffer, to: &mut AlarmStatesBuffer) {
        to.states = from.states;
    }
}
