//! Step two: how far the board's clock is ahead of the camera's over the recording, as the median of the pairs' offsets
//! around every `step` seconds of the board's time (within `around` seconds either side); and, from those, any camera
//! time put on the board's clock, the offset taken as changing steadily between them.

use super::pairs::Pair;

/// The offset at a moment of the board's clock: the median of the pairs' around it, if there are any.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Knot {
    /// Seconds, from the start of recording.
    pub board: f64,
    /// Seconds the board's clock is ahead of the camera's.
    pub offset: Option<f64>,
    /// The pairs the median's of.
    pub pairs: usize,
}

/// The offsets every `step` seconds from the start of recording until `until` is covered, from `pairs`, in the board's
/// time order.
pub fn knots(pairs: &[Pair], step: f64, around: f64, until: f64) -> Vec<Knot> {
    let count = (until.max(0.0) / step).ceil() as usize;
    (0..=count)
        .map(|k| {
            let board = k as f64 * step;
            let from = pairs.partition_point(|p| p.board < board - around);
            let to = pairs.partition_point(|p| p.board <= board + around);
            let mut offsets: Vec<f64> = pairs[from..to].iter().map(Pair::offset).collect();
            Knot { board, offset: median(&mut offsets), pairs: offsets.len() }
        })
        .collect()
}

pub fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle]) / 2.0 })
}

/// The camera's clock put on the board's, by the knots with an offset: between them the offset changes steadily, and
/// before the first and after the last it's theirs.
#[derive(Clone, PartialEq, Debug)]
pub struct Clock {
    /// (board seconds, offset), in time order.
    points: Vec<(f64, f64)>,
}

impl Clock {
    /// None if no knot has an offset.
    pub fn new(knots: &[Knot]) -> Option<Self> {
        let points: Vec<_> = knots.iter().filter_map(|k| Some((k.board, k.offset?))).collect();
        (!points.is_empty()).then_some(Clock { points })
    }

    /// The offset at `board` seconds of the board's clock.
    pub fn offset_at(&self, board: f64) -> f64 {
        interpolate(&self.points, board)
    }

    /// `led` seconds of the camera's clock, on the board's: where the board's time less the offset there is `led`.
    pub fn to_board(&self, led: f64) -> f64 {
        // By the camera's time at each knot: it goes up as the board's does, as offsets change far slower than time.
        let by_camera = self.points.iter().map(|&(board, offset)| (board - offset, offset));
        led + interpolate(&by_camera.collect::<Vec<_>>(), led)
    }
}

/// `points`' y at `x`, along straight lines between them, and level beyond them.
fn interpolate(points: &[(f64, f64)], x: f64) -> f64 {
    let after = points.partition_point(|&(px, _)| px <= x);
    match (after.checked_sub(1).map(|i| points[i]), points.get(after)) {
        (Some((x0, y0)), Some(&(x1, y1))) if x1 > x0 => y0 + (y1 - y0) * (x - x0) / (x1 - x0),
        (Some((_, y)), _) | (None, Some(&(_, y))) => y,
        (None, None) => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(board: f64, offset: f64) -> Pair {
        Pair { board, led: board - offset }
    }

    #[test]
    fn each_knot_is_the_median_of_the_pairs_around_it() {
        let pairs = [pair(0.0, 0.010), pair(20.0, 0.011), pair(25.0, 0.500), pair(70.0, 0.012), pair(300.0, 0.020)];
        let knots = knots(&pairs, 30.0, 60.0, 200.0);
        let boards: Vec<f64> = knots.iter().map(|k| k.board).collect();
        assert_eq!(boards, [0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0], "covering 200 s");
        let near = |knot: &Knot, offset: f64| (knot.offset.unwrap() - offset).abs() < 1e-12;
        assert!(near(&knots[0], 0.011), "the outlier's outvoted");
        assert_eq!(knots[1].pairs, 4);
        assert!(near(&knots[1], 0.0115), "between the middle two");
        assert_eq!((knots[5].offset, knots[5].pairs), (None, 0), "nothing within a minute");
    }

    #[test]
    fn camera_times_go_on_the_board_s_clock_by_the_offsets_either_side() {
        let knot = |board, offset| Knot { board, offset, pairs: 1 };
        let knots = [knot(0.0, Some(0.1)), knot(30.0, None), knot(60.0, Some(0.4))];
        let clock = Clock::new(&knots).unwrap();
        assert!((clock.offset_at(30.0) - 0.25).abs() < 1e-12, "across the knot without one");
        assert_eq!(clock.offset_at(-5.0), 0.1);
        assert_eq!(clock.offset_at(100.0), 0.4, "level beyond");
        // 29.75 s of the camera's is 30 s of the board's, where the offset's 0.25 s.
        assert!((clock.to_board(29.75) - 30.0).abs() < 1e-9);
        assert_eq!(clock.to_board(100.0), 100.4);
        assert!(Clock::new(&[knot(0.0, None)]).is_none());
    }
}
