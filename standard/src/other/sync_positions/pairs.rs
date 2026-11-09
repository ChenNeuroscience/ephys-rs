//! Step one: pairing the timing LED's changes, as the camera saw them, with the same changes on the board's line.
//!
//! A change on either side pairs with one the same way (up or down) on the other within `within` seconds (by the clocks'
//! own times: they start at much the same moment), if that's the only one either side has within `alone` seconds;
//! otherwise it's ambiguous (it had one within `within`, but not alone) or unmatched (it had none).

use super::files::Change;

/// The same change, by each clock: in seconds, the board's from the start of recording, the camera's from the first
/// position.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Pair {
    pub board: f64,
    pub led: f64,
}

impl Pair {
    /// How far the board's clock is ahead of the camera's, in seconds.
    pub fn offset(&self) -> f64 {
        self.board - self.led
    }
}

/// What became of one side's changes.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Tally {
    pub paired: usize,
    pub ambiguous: usize,
    pub unmatched: usize,
}

impl Tally {
    pub fn changes(&self) -> usize {
        self.paired + self.ambiguous + self.unmatched
    }
}

/// The pairs found, in the board's time order, and what became of each side's changes.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Pairing {
    pub pairs: Vec<Pair>,
    pub board: Tally,
    pub led: Tally,
}

/// Pairs `board`'s changes (already just the line used) with `led`'s.
pub fn pair(board: &[Change], led: &[Change], within: f64, alone: f64) -> Pairing {
    let mut pairing = Pairing::default();
    for up in [true, false] {
        let times = |changes: &[Change]| {
            let mut times: Vec<f64> = changes.iter().filter(|c| c.up == up).map(|c| c.seconds).collect();
            times.sort_by(f64::total_cmp);
            times
        };
        let (b, l) = (times(board), times(led));
        let (b_to_l, l_to_b) = (candidates(&b, &l, within, alone), candidates(&l, &b, within, alone));
        for (i, found) in b_to_l.iter().enumerate() {
            match found {
                Found::Only(j) if l_to_b[*j] == Found::Only(i) => {
                    pairing.pairs.push(Pair { board: b[i], led: l[*j] });
                    pairing.board.paired += 1;
                    pairing.led.paired += 1;
                }
                Found::None => pairing.board.unmatched += 1,
                _ => pairing.board.ambiguous += 1,
            }
        }
        for (j, found) in l_to_b.iter().enumerate() {
            match found {
                Found::Only(i) if b_to_l[*i] == Found::Only(j) => {}
                Found::None => pairing.led.unmatched += 1,
                _ => pairing.led.ambiguous += 1,
            }
        }
    }
    pairing.pairs.sort_by(|a, b| a.board.total_cmp(&b.board));
    pairing
}

/// What a change found on the other side.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Found {
    None,
    /// The one change within `within`, and none other within `alone`.
    Only(usize),
    Several,
}

/// What each of `from` (sorted) finds in `to` (sorted).
fn candidates(from: &[f64], to: &[f64], within: f64, alone: f64) -> Vec<Found> {
    let range = |t: f64, by: f64| to.partition_point(|&x| x < t - by)..to.partition_point(|&x| x <= t + by);
    from.iter()
        .map(|&t| {
            let near = range(t, within);
            if near.is_empty() {
                Found::None
            } else if range(t, alone.max(within)).len() == 1 {
                Found::Only(near.start)
            } else {
                Found::Several
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(seconds: f64, up: bool) -> Change {
        Change { line: 1, seconds, up }
    }

    #[test]
    fn changes_pair_with_the_only_one_near_them_going_the_same_way() {
        let board = [change(10.0, true), change(10.5, false), change(40.0, true), change(70.0, true)];
        let led = [change(10.02, true), change(10.52, false), change(40.3, true), change(41.5, true)];
        let pairing = pair(&board, &led, 1.0, 2.0);
        let pairs: Vec<_> = pairing.pairs.iter().map(|p| (p.board, p.led)).collect();
        assert_eq!(pairs, [(10.0, 10.02), (10.5, 10.52)], "up with up, down with down");
        assert_eq!(pairing.board, Tally { paired: 2, ambiguous: 1, unmatched: 1 }, "40 s has two within 2 s");
        assert_eq!(pairing.led, Tally { paired: 2, ambiguous: 1, unmatched: 1 }, "41.5 s has none within 1 s");
    }

    #[test]
    fn a_change_alone_on_one_side_but_not_the_other_is_ambiguous() {
        // The LED's change sees two of the board's within 2 s: neither pairs, though the first sees only it.
        let board = [change(5.0, true), change(6.5, true)];
        let led = [change(5.1, true)];
        let pairing = pair(&board, &led, 1.0, 2.0);
        assert!(pairing.pairs.is_empty());
        assert_eq!(pairing.board, Tally { paired: 0, ambiguous: 1, unmatched: 1 });
        assert_eq!(pairing.led, Tally { paired: 0, ambiguous: 1, unmatched: 0 });
        assert_eq!(pair(&board, &led, 1.0, 1.0).pairs.len(), 1, "alone within 1 s");
    }
}
