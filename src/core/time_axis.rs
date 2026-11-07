//! How every plot of data over time lays out its time axis, so they all read the same way: time runs left to right, and
//! the axis stays put, showing how long ago things were, with now just short of the right edge, leaving a small gap
//! there for the future, which has no data yet. The data comes in there and moves left.

/// The gap left at the right for the future, as a fraction of the span shown.
pub const FUTURE: f64 = 0.03;

/// The times at the left and right edges of a plot showing `span` seconds up to `now`.
pub fn window(now: f64, span: f64) -> (f64, f64) {
    (now - span, now + span * FUTURE)
}

/// How a time on such an axis reads, wherever it's drawn: now, or `seconds` ago as a negative, to the millisecond.
pub fn label(seconds: f64) -> String {
    let rounded = (seconds * 1000.0).round() / 1000.0;
    if rounded == 0.0 { "now".into() } else { format!("{rounded} s").replace('-', "−") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_the_same_whatever_the_step() {
        assert_eq!([-10.0, -1.5, -0.25, 0.0, -0.0].map(label), ["−10 s", "−1.5 s", "−0.25 s", "now", "now"]);
        assert_eq!(label(-0.30000000000000004), "−0.3 s", "whatever a step's sums come to");
    }

    #[test]
    fn ends_at_now_with_room_for_the_future() {
        let (left, right) = window(30.0, 10.0);
        assert_eq!(left, 20.0);
        assert!((right - 30.3).abs() < 1e-9, "now just short of the right");
        assert_eq!(window(0.0, 10.0).0, -10.0, "seconds ago, as an axis");
    }
}
