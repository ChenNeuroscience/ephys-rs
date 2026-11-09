//! Putting positions on the board's clock: from the timing LED's changes, seen by the camera (`name.pos.ttl`), and the
//! same pulses on one of the board's digital inputs (`name.ttl`).
//!
//! For now, the positions' timestamps are left as they are: the algorithm's to come.

use super::SyncSettings;
use super::files::{Change, LtxFile, POS_RECORD, timebase, timestamp};

/// What syncing found, to show.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Stats {
    pub positions: usize,
    /// How long the positions last, in seconds.
    pub positions_seconds: f64,
    /// The changes of the board's line used.
    pub board_changes: usize,
    pub led_changes: usize,
}

/// `pos`, with its timestamps on the board's clock, and what was found doing it.
pub fn sync(
    pos: &LtxFile,
    board: &[Change],
    led: &[Change],
    settings: &SyncSettings,
) -> Result<(LtxFile, Stats), String> {
    if !pos.data.len().is_multiple_of(POS_RECORD) {
        return Err(format!("The positions aren't whole records of {POS_RECORD} bytes"));
    }
    let timebase = timebase(pos)?;
    let records: Vec<&[u8]> = pos.data.chunks_exact(POS_RECORD).collect();
    let span = match (records.first(), records.last()) {
        (Some(first), Some(last)) => (timestamp(last) as f64 - timestamp(first) as f64) / timebase,
        _ => 0.0,
    };
    let stats = Stats {
        positions: records.len(),
        positions_seconds: span,
        board_changes: board.iter().filter(|c| c.line == settings.board_line).count(),
        led_changes: led.len(),
    };
    Ok((pos.clone(), stats))
}
