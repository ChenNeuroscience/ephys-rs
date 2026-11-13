//! Medians across rows of samples: at each sample, the median of the rows' values there (a common median reference's,
//! say). For a few rows (up to `NETWORK_UP_TO`), by a sorting network, `LANES` samples at once; for more, by the
//! standard library's selection, a sample at a time.
//!
//! A sorting network is a fixed list of compare-and-swaps, each putting the lower of two positions' values in the first
//! and the higher in the second (a min and a max). The list depends only on how many values there are, never on what
//! they are, so there are no branches to mispredict. And each position holds `LANES` samples of one row, loaded as they
//! lie, so each compare-and-swap does all of them at once. The standard library's selection sorts 16 or fewer values by
//! insertion, whose branches go however the data does, so for a few rows the network's several times quicker (about 4×
//! for 4 rows, 3× for 32). A network's compare-and-swaps grow as n log² n, though, and the standard library's work as
//! n, and its partitioning is branch-free: for many rows it's as quick or quicker.
//!
//! The network's Batcher's odd-even merge sort, for the next power of two, its extra inputs +∞. Those are known, so
//! they're taken out when it's made: a compare-and-swap with +∞ second does nothing, and one with +∞ first only swaps
//! which input's at which position. Then only the compare-and-swaps that lead to the middle position are kept.
//!
//! Both give the upper of the middle two of an even number of rows. They can differ only in edge cases: the network's
//! min and max take the other value over a NaN, where the standard library's order (`total_cmp`) puts NaN at the ends;
//! and the network may give −0 where it gives +0, or the other way about.

/// The most rows medians are found by a network for.
pub const NETWORK_UP_TO: usize = 32;

/// Samples a network works on at once.
const LANES: usize = 8;

/// Finds the medians across a number of rows.
pub struct Medians {
    rows: usize,
    /// For a network: its compare-and-swaps (pairs of inputs), and the input its median ends at.
    network: Option<(Vec<(u8, u8)>, usize)>,
    /// Room to work in: each input's samples, for a network; a sample's values, for the standard library.
    lanes: Vec<[f32; LANES]>,
    values: Vec<f32>,
}

impl Medians {
    pub fn new(rows: usize) -> Self {
        let network = (rows > 0 && rows <= NETWORK_UP_TO).then(|| network(rows));
        let lanes = vec![[0.0; LANES]; if network.is_some() { rows } else { 0 }];
        Medians { rows, network, lanes, values: Vec::with_capacity(rows) }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Each sample's median across the rows, into `out`: `row(i)` is row `i`, at least as long as `out`. Zero for each,
    /// if there are no rows.
    pub fn find<'a>(&mut self, out: &mut [f32], row: impl Fn(usize) -> &'a [f32]) {
        if self.rows == 0 {
            out.fill(0.0);
            return;
        }
        let Some((network, middle)) = &self.network else {
            let middle = self.rows / 2;
            for (t, out) in out.iter_mut().enumerate() {
                self.values.clear();
                self.values.extend((0..self.rows).map(|i| row(i)[t]));
                *out = *self.values.select_nth_unstable_by(middle, f32::total_cmp).1;
            }
            return;
        };
        let lanes = &mut self.lanes;
        for (start, out) in (0..).step_by(LANES).zip(out.chunks_mut(LANES)) {
            let n = out.len();
            for (i, lane) in lanes.iter_mut().enumerate() {
                lane[..n].copy_from_slice(&row(i)[start..start + n]);
            }
            for &(a, b) in network {
                let (a, b) = (a as usize, b as usize);
                let (x, y) = (lanes[a], lanes[b]);
                let (mut low, mut high) = ([0.0; LANES], [0.0; LANES]);
                for l in 0..LANES {
                    (low[l], high[l]) = (x[l].min(y[l]), x[l].max(y[l]));
                }
                (lanes[a], lanes[b]) = (low, high);
            }
            out.copy_from_slice(&lanes[*middle][..n]);
        }
    }
}

/// The compare-and-swaps for the median of `inputs` values, as pairs of inputs (the lower value to the first), and the
/// input it ends at.
fn network(inputs: usize) -> (Vec<(u8, u8)>, usize) {
    // Which input's at each position of the padded network, as it moves them (`None` for +∞).
    let mut at: Vec<Option<usize>> = (0..inputs.next_power_of_two()).map(|p| (p < inputs).then_some(p)).collect();
    let mut network = Vec::new();
    for (a, b) in batcher(at.len()) {
        match (at[a], at[b]) {
            (_, None) => {}                    // in order already
            (None, Some(_)) => at.swap(a, b), // the input comes down, the +∞ goes up
            (Some(x), Some(y)) => network.push((x, y)),
        }
    }
    let middle = at[inputs / 2].expect("the +∞ end up above all the inputs");
    // Only what leads to the middle, working back from it.
    let mut needed = vec![false; inputs];
    needed[middle] = true;
    let mut kept: Vec<(u8, u8)> = Vec::new();
    for &(x, y) in network.iter().rev() {
        if needed[x] || needed[y] {
            (needed[x], needed[y]) = (true, true);
            kept.push((x as u8, y as u8));
        }
    }
    kept.reverse();
    (kept, middle)
}

/// Batcher's odd-even merge sort of `size` values (a power of two): pairs of positions, each to be put in order, in turn.
fn batcher(size: usize) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut p = 1;
    while p < size {
        let mut k = p;
        while k >= 1 {
            for j in (k % p..size - k).step_by(2 * k) {
                for i in 0..k.min(size - j - k) {
                    if (i + j) / (2 * p) == (i + j + k) / (2 * p) {
                        pairs.push((i + j, i + j + k));
                    }
                }
            }
            k /= 2;
        }
        p *= 2;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rows` rows of `len` samples, from as many different values as `spread` (few, for many ties).
    fn rows(rows: usize, len: usize, spread: u64, seed: u64) -> Vec<Vec<f32>> {
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % spread) as f32 - (spread / 2) as f32
        };
        (0..rows).map(|_| (0..len).map(|_| next()).collect()).collect()
    }

    /// Each sample's median, by sorting.
    fn sorted_medians(rows: &[Vec<f32>]) -> Vec<f32> {
        (0..rows[0].len())
            .map(|t| {
                let mut values: Vec<f32> = rows.iter().map(|row| row[t]).collect();
                values.sort_by(f32::total_cmp);
                values[values.len() / 2]
            })
            .collect()
    }

    #[test]
    fn medians_are_the_middle_values_either_way() {
        for count in (1..=NETWORK_UP_TO + 8).chain([63, 64, 65]) {
            for (len, spread) in [(1, 1000), (8, 1000), (37, 1000), (600, 100_000), (100, 3)] {
                let rows = rows(count, len, spread, (count * 1000 + len) as u64);
                let mut out = vec![f32::NAN; len];
                Medians::new(count).find(&mut out, |i| &rows[i]);
                assert_eq!(out, sorted_medians(&rows), "{count} rows of {len}, from {spread} values");
            }
        }
    }

    #[test]
    fn rows_can_be_longer_than_whats_wanted() {
        let rows = rows(5, 20, 1000, 1);
        let mut out = vec![0.0; 13];
        Medians::new(5).find(&mut out, |i| &rows[i]);
        assert_eq!(out, sorted_medians(&rows)[..13]);
    }

    #[test]
    fn no_rows_is_zeros() {
        let mut out = vec![1.0; 4];
        Medians::new(0).find(&mut out, |_| &[]);
        assert_eq!(out, [0.0; 4]);
    }

    #[test]
    fn networks_keep_only_what_the_middle_needs() {
        // As few as the best known for 3 and 4; for 9, 24 of the padded 63 (the best known is 19).
        let sizes: Vec<usize> = [3, 4, 9, 16, 32].map(|n| network(n).0.len()).into();
        assert_eq!(sizes, [3, 5, 24, 53, 157]);
        assert!(batcher(16).len() == 63 && batcher(32).len() == 191);
    }
}
