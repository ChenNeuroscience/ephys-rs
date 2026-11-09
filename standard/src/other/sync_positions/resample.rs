//! Step three: the positions, on the board's clock, one in each slot of the rate they say they're at (`sample_rate`),
//! from the start of recording. Positions aren't interpolated: each goes in the slot nearest its time; where several do,
//! the nearest is kept and the rest dropped, and a slot none go in is filled with a position not found (no LEDs).

use super::files::{POS_RECORD, set_timestamp, timestamp};
use super::offsets::Clock;

/// What resampling did.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Edits {
    /// Positions written, kept and added.
    pub positions: usize,
    /// Dropped for sharing a slot with a nearer one.
    pub dropped: usize,
    /// Dropped for being before the start of recording.
    pub before: usize,
    /// Added in empty slots, and the runs of them, and the longest.
    pub added: usize,
    pub gaps: usize,
    pub longest_gap: usize,
}

/// `data`'s positions (whole records, timed in `timebase` ticks on the camera's clock), put on the board's by `clock`, at
/// `rate` a second; positions not found have their coordinates `missing`.
pub fn resample(data: &[u8], timebase: f64, clock: &Clock, rate: f64, missing: u16) -> (Vec<u8>, Edits) {
    let mut edits = Edits::default();
    // For each slot, the position in it and how far from the slot's time it was.
    let mut slots: Vec<Option<(&[u8], f64)>> = Vec::new();
    for record in data.chunks_exact(POS_RECORD) {
        let board = clock.to_board(timestamp(record) as f64 / timebase);
        let slot = (board * rate).round();
        if slot < 0.0 {
            edits.before += 1;
            continue;
        }
        let (slot, off) = (slot as usize, (board * rate - slot).abs());
        if slots.len() <= slot {
            slots.resize(slot + 1, None);
        }
        match &slots[slot] {
            Some((_, nearer)) if *nearer <= off => edits.dropped += 1,
            Some(_) => {
                edits.dropped += 1;
                slots[slot] = Some((record, off));
            }
            None => slots[slot] = Some((record, off)),
        }
    }
    let mut out = Vec::with_capacity(slots.len() * POS_RECORD);
    let mut gap = 0;
    for (slot, found) in slots.iter().enumerate() {
        let start = out.len();
        match found {
            Some((record, _)) => {
                out.extend_from_slice(record);
                gap = 0;
            }
            None => {
                out.extend_from_slice(&not_found(missing));
                edits.added += 1;
                if gap == 0 {
                    edits.gaps += 1;
                }
                gap += 1;
                edits.longest_gap = edits.longest_gap.max(gap);
            }
        }
        let ticks = (slot as f64 / rate * timebase).round();
        set_timestamp(&mut out[start..], ticks.min(u32::MAX as f64) as u32);
    }
    edits.positions = slots.len();
    (out, edits)
}

/// A position record with no LEDs found: its coordinates `missing`, its sizes 0.
fn not_found(missing: u16) -> [u8; POS_RECORD] {
    let mut record = [0; POS_RECORD];
    for value in 0..4 {
        record[4 + value * 2..6 + value * 2].copy_from_slice(&missing.to_be_bytes());
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::other::sync_positions::offsets::Knot;

    /// Positions at `ticks` (of 1 kHz), their first value the position's number.
    fn positions(ticks: &[u32]) -> Vec<u8> {
        let mut data = Vec::new();
        for (i, &t) in ticks.iter().enumerate() {
            data.extend_from_slice(&t.to_be_bytes());
            data.extend_from_slice(&(i as u16).to_be_bytes());
            data.extend_from_slice(&[0; POS_RECORD - 6]);
        }
        data
    }

    fn clock(offset: f64) -> Clock {
        Clock::new(&[Knot { board: 0.0, offset: Some(offset), pairs: 1 }]).unwrap()
    }

    /// Each slot's timestamp and first value.
    fn read(data: &[u8]) -> Vec<(u32, u16)> {
        data.chunks_exact(POS_RECORD).map(|r| (timestamp(r), u16::from_be_bytes([r[4], r[5]]))).collect()
    }

    #[test]
    fn each_position_goes_in_its_nearest_slot_and_none_are_made_up() {
        let data = positions(&[0, 100, 200, 210, 500]);
        let (out, edits) = resample(&data, 1000.0, &clock(-0.08), 10.0, 1023);
        // By the board's: -0.08, 0.02, 0.12, 0.13, 0.42 s: slots before, 0, 1, 1 (further), 4.
        assert_eq!(read(&out), [(0, 1), (100, 2), (200, 1023), (300, 1023), (400, 4)]);
        let expected = Edits { positions: 5, dropped: 1, before: 1, added: 2, gaps: 1, longest_gap: 2 };
        assert_eq!(edits, expected);
        let added = &out[2 * POS_RECORD..3 * POS_RECORD];
        assert_eq!(&added[4..12], &[3, 255, 3, 255, 3, 255, 3, 255], "no LEDs");
        assert_eq!(&added[12..], &[0; 8], "of no size");
    }

    #[test]
    fn of_two_in_a_slot_the_nearer_is_kept_whichever_came_first() {
        let data = positions(&[0, 160, 190]);
        let (out, edits) = resample(&data, 1000.0, &clock(0.0), 10.0, 1023);
        assert_eq!(read(&out), [(0, 0), (100, 1023), (200, 2)]);
        assert_eq!(edits.dropped, 1);
        let data = positions(&[0, 190, 240]);
        assert_eq!(read(&resample(&data, 1000.0, &clock(0.0), 10.0, 1023).0), [(0, 0), (100, 1023), (200, 1)]);
    }
}
