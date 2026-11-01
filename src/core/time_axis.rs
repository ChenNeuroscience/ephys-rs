//! How every plot of data over time lays out its time axis, so they all read the same way: time runs left to
//! right; at first the data fills from the left edge, and once there's a full span of it, it scrolls, the newest
//! just short of the right edge, leaving a small gap there for the future, which has no data yet.

/// The gap left at the right for the future, as a fraction of the span shown.
pub const FUTURE: f64 = 0.03;

/// The times at the left and right edges of a plot showing `span` seconds, from data that began at `start` and
/// runs to `now`.
pub fn window(start: f64, now: f64, span: f64) -> (f64, f64) {
    let left = start.max(now - span);
    (left, left + span * (1.0 + FUTURE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_from_the_left_then_scrolls_with_room_for_the_future() {
        assert_eq!(window(2.0, 5.0, 10.0), (2.0, 12.3), "filling from the left");
        let (left, right) = window(2.0, 30.0, 10.0);
        assert_eq!(left, 20.0, "scrolling");
        assert!((right - 30.3).abs() < 1e-9, "the newest just short of the right");
    }
}
