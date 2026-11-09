//! What's worth warning about in a sync that otherwise worked: the problems that stop one are said where they're found.

use super::Synced;
use super::offsets::Knot;

/// Less of the timing LED's changes paired than this is worth a warning.
const PAIRED: f64 = 0.5;
/// Clocks drifting apart by more than this (parts per million) is more than crystals do.
const DRIFT_PPM: f64 = 500.0;
/// Positions dropped or added, as a share of those written, worth a warning.
const EDITED: f64 = 0.01;

/// The warnings, worst first.
pub fn warnings(synced: &Synced) -> Vec<String> {
    let mut warnings = Vec::new();
    // Half a frame: what the timing LED can be timed to.
    let half_frame = 0.5 / synced.rate;
    let ms = |seconds: f64| format!("{:.1} ms", seconds * 1000.0);
    let jump = synced.knots.iter().filter_map(|k| Some((k.board, k.offset?))).collect::<Vec<_>>();
    let biggest = jump.windows(2).map(|w| (w[0].0, w[1].0, (w[1].1 - w[0].1).abs())).max_by(|a, b| a.2.total_cmp(&b.2));
    if let Some((from, to, by)) = biggest.filter(|b| b.2 > half_frame) {
        warnings.push(format!(
            "The offset jumps by {} between {from:.0} s and {to:.0} s, more than half a frame: did a clock restart?",
            ms(by)
        ));
    }
    if synced.scatter > half_frame {
        warnings.push(format!(
            "The pairs are {} from the offset, typically, more than half a frame: some may be wrongly paired",
            ms(synced.scatter)
        ));
    }
    if let Some(ppm) = synced.drift_ppm.filter(|ppm| ppm.abs() > DRIFT_PPM) {
        warnings.push(format!("The clocks drift apart by {ppm:.0} ppm, far more than crystals do"));
    }
    let led = synced.pairing.led;
    if (led.paired as f64) < PAIRED * led.changes() as f64 {
        warnings.push(format!(
            "Only {} of the timing LED's {} changes paired ({} ambiguous, {} unmatched)",
            led.paired,
            led.changes(),
            led.ambiguous,
            led.unmatched
        ));
    }
    if let Some((from, to)) = longest_without(&synced.knots) {
        let when = if from == to { format!("around {from:.0} s") } else { format!("from {from:.0} s to {to:.0} s") };
        warnings.push(format!("No pairs {when} of the recording: the offset there is estimated from either side"));
    }
    let edits = synced.edits;
    if (edits.dropped + edits.added) as f64 > EDITED * edits.positions as f64 {
        warnings.push(format!(
            "{} positions were dropped and {} added, of {}: is the camera at the rate it says?",
            edits.dropped, edits.added, edits.positions
        ));
    }
    if edits.before as f64 > synced.rate {
        warnings.push(format!("{} positions from before the start of recording were dropped", edits.before));
    }
    warnings
}

/// The longest run of knots without an offset: the board's times of its first and last.
fn longest_without(knots: &[Knot]) -> Option<(f64, f64)> {
    let mut longest: Option<(f64, f64)> = None;
    let mut start = None;
    for knot in knots {
        if knot.offset.is_some() {
            start = None;
            continue;
        }
        let from = *start.get_or_insert(knot.board);
        if longest.is_none_or(|(a, b)| knot.board - from > b - a) {
            longest = Some((from, knot.board));
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::super::pairs::{Pairing, Tally};
    use super::super::resample::Edits;
    use super::*;

    /// A clean sync, at 50 Hz: nothing to warn of.
    fn clean() -> Synced {
        let knot = |board, offset| Knot { board, offset: Some(offset), pairs: 10 };
        let tally = Tally { paired: 90, ambiguous: 5, unmatched: 5 };
        Synced {
            recorded: 3000,
            rate: 50.0,
            pairing: Pairing { pairs: Vec::new(), board: tally, led: tally },
            knots: vec![knot(0.0, 0.005), knot(30.0, 0.006), knot(60.0, 0.007)],
            drift_ppm: Some(30.0),
            scatter: 0.004,
            edits: Edits { positions: 3000, dropped: 10, added: 10, ..Default::default() },
            warnings: Vec::new(),
        }
    }

    fn warned(change: impl FnOnce(&mut Synced)) -> Vec<String> {
        let mut synced = clean();
        change(&mut synced);
        warnings(&synced)
    }

    #[test]
    fn a_clean_sync_has_nothing_to_warn_of() {
        assert!(warnings(&clean()).is_empty(), "{:?}", warnings(&clean()));
    }

    #[test]
    fn each_check_warns_past_its_limit() {
        let one = |change: fn(&mut Synced), says: &str| {
            let warnings = warned(change);
            assert!(warnings.len() == 1 && warnings[0].contains(says), "{says}: {warnings:?}");
        };
        one(|s| s.knots[2].offset = Some(0.020), "jumps by 14.0 ms between 30 s and 60 s");
        one(|s| s.scatter = 0.011, "11.0 ms from the offset");
        one(|s| s.drift_ppm = Some(-600.0), "-600 ppm");
        one(|s| s.pairing.led = Tally { paired: 49, ambiguous: 1, unmatched: 50 }, "Only 49 of the timing LED's 100");
        one(|s| s.knots[1].offset = None, "No pairs around 30 s");
        one(|s| s.edits.added = 21, "10 positions were dropped and 21 added, of 3000");
        one(|s| s.edits.before = 51, "51 positions from before");
    }

    #[test]
    fn the_longest_run_without_pairs_is_said() {
        let knot = |board, offset| Knot { board, offset, pairs: 0 };
        let knots = [knot(0.0, None), knot(30.0, Some(0.0)), knot(60.0, None), knot(90.0, None), knot(120.0, None)];
        assert_eq!(longest_without(&knots), Some((60.0, 120.0)));
        assert_eq!(longest_without(&knots[1..2]), None);
    }
}
