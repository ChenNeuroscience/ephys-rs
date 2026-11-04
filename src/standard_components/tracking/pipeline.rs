//! Finding LEDs in a frame: bright regions within the crop, of a size within limits, largest first. Mirrors
//! the Bonsai workflow it replaced (Grayscale → Crop → Threshold → FindContours → BinaryRegionAnalysis →
//! SortBinaryRegions), without building per-frame images.

use super::{Crop, Detection};

/// A bright region: its centroid (in pixel indices) and area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blob {
    pub x: f32,
    pub y: f32,
    pub area: u32,
}

/// Finds bright regions: each row of the crop is scanned for runs of above-threshold pixels, and runs that
/// touch (8-connected) across rows are merged with union-find while their area and centroid sums are
/// accumulated. Most rows contain no LED at all, and those are rejected with a single (SIMD-friendly) max
/// over the row. Keeps its working memory between frames.
#[derive(Default)]
pub struct Pipeline {
    spans: SpanCache,
    runs: Vec<Run>,
    /// Union-find parent of each run.
    parent: Vec<u32>,
    blobs: Vec<Blob>,
}

/// A horizontal run of foreground pixels, carrying the totals of its region once merged.
#[derive(Clone, Copy)]
struct Run {
    x0: u32,
    /// Exclusive.
    x1: u32,
    area: u32,
    sum_x: u64,
    sum_y: u64,
}

/// The crop as one span of columns per row (crops are convex, so the inside of a row is contiguous), rebuilt
/// only when the crop, edge width or frame size changes.
#[derive(Default)]
struct SpanCache {
    key: Option<(Crop, f32, Option<Crop>, u32, u32)>,
    spans: Vec<RowSpans>,
}

/// Columns of one row, as start..end (empty if start >= end).
#[derive(Clone, Copy)]
pub struct RowSpans {
    /// Inside the crop.
    pub outer: (u32, u32),
    /// Inside the crop and outside the edge band (all of `outer` when there's no band).
    pub inner: (u32, u32),
    /// Inside the timing LED's spot, which isn't looked at for the other LEDs.
    pub timing: (u32, u32),
}

impl SpanCache {
    fn get(&mut self, detection: &Detection, width: u32, height: u32) -> &[RowSpans] {
        let key = (detection.crop, detection.edge_width(), detection.timing_spot(), width, height);
        if self.key != Some(key) {
            self.spans = (0..height).map(|y| row_spans(detection, y, width)).collect();
            self.key = Some(key);
        }
        &self.spans
    }
}

/// The crop's spans for row `y`, those inside the edge band, and the timing LED's spot.
pub fn row_spans(detection: &Detection, y: u32, width: u32) -> RowSpans {
    let (crop, edge_width) = (&detection.crop, detection.edge_width());
    let outer = row_span(crop, y, width);
    let inner = if edge_width <= 0.0 || *crop == Crop::None {
        outer
    } else {
        crop.inset(edge_width).map_or((0, 0), |inset| row_span(&inset, y, width))
    };
    let timing = detection.timing_spot().map_or((0, 0), |spot| row_span(&spot, y, width));
    RowSpans { outer, inner, timing }
}

/// Whether the timing LED is on in a `width` × `height` frame of grey `pixels`, if there is one: enough of its spot is
/// bright enough, wherever the crop is.
pub fn timing_led_on(pixels: &[u8], width: u32, height: u32, detection: &Detection) -> Option<bool> {
    let spot = detection.timing_spot()?;
    let threshold = detection.timing_threshold();
    let [center, radius] = [detection.timing_led.center[1], detection.timing_led.radius];
    let rows = (center - radius).floor().max(0.0) as u32..((center + radius).ceil().max(0.0) as u32 + 1).min(height);
    let bright: usize = rows
        .map(|y| {
            let (start, end) = row_span(&spot, y, width);
            let row = (y * width) as usize;
            pixels[row + start as usize..row + end as usize].iter().filter(|&&value| value > threshold).count()
        })
        .sum();
    Some(bright >= detection.timing_min_area() as usize)
}

/// Columns of row `y` whose pixel centres are inside `crop`, as start..end (empty if start >= end). Worked
/// out from the shape rather than by testing every pixel, so it's cheap enough to redo on every frame while
/// a crop is being dragged; the ends are then checked with [`Crop::contains`] so they agree with it exactly.
pub fn row_span(crop: &Crop, y: u32, width: u32) -> (u32, u32) {
    let yc = y as f32 + 0.5;
    let (lo, hi) = match *crop {
        Crop::None => return (0, width),
        Crop::Circle { center, radius } => {
            let dy = yc - center[1];
            if dy.abs() > radius {
                return (0, 0);
            }
            let dx = (radius * radius - dy * dy).sqrt();
            (center[0] - dx, center[0] + dx)
        }
        Crop::Square { .. } | Crop::Rectangle { .. } => {
            let (center, [hw, hh]) = (crop.center().unwrap(), crop.half_extents().unwrap());
            let (sin, cos) = crop.angle().sin_cos();
            let dy = yc - center[1];
            // In the shape's frame, local x = dx*cos + dy*sin and local y = -dx*sin + dy*cos (see to_local);
            // each must lie within its half extent, which bounds dx.
            let (mut lo, mut hi) = (f32::NEG_INFINITY, f32::INFINITY);
            for (slope, offset, half) in [(cos, dy * sin, hw), (-sin, dy * cos, hh)] {
                if slope.abs() < 1e-6 {
                    if offset.abs() > half {
                        return (0, 0);
                    }
                } else {
                    let (a, b) = ((-half - offset) / slope, (half - offset) / slope);
                    lo = lo.max(a.min(b));
                    hi = hi.min(a.max(b));
                }
            }
            (center[0] + lo, center[0] + hi)
        }
    };
    let inside = |x: u32| crop.contains([x as f32 + 0.5, yc]);
    let clamp = |v: f32| v.clamp(0.0, width as f32) as u32;
    let (mut start, mut end) = (clamp((lo - 0.5).ceil()), clamp((hi - 0.5).floor() + 1.0));
    // Float rounding can leave the ends a pixel out from the exact test; nudge them to agree.
    while start < end && !inside(start) {
        start += 1;
    }
    while start > 0 && start <= end && inside(start - 1) {
        start -= 1;
    }
    while end > start && !inside(end - 1) {
        end -= 1;
    }
    while end > start && end < width && inside(end) {
        end += 1;
    }
    if start < end { (start, end) } else { (0, 0) }
}

impl Pipeline {
    /// The blobs in a `width` × `height` frame of grey `pixels` passing the area limits, largest first.
    pub fn process(&mut self, pixels: &[u8], width: u32, height: u32, detection: &Detection) -> &[Blob] {
        let spans = self.spans.get(detection, width, height);
        let threshold = detection.threshold;
        let edge_threshold = detection.edge_threshold(); // >= threshold
        let (runs, parent) = (&mut self.runs, &mut self.parent);
        runs.clear();
        parent.clear();

        let mut prev_row = 0..0; // indices of the previous row's runs
        for (y, spans) in spans.iter().enumerate() {
            let first_run = runs.len();
            let (start, end) = spans.outer;
            if start < end {
                let offset = y * width as usize;
                let row = &pixels[offset + start as usize..offset + end as usize];
                if row.iter().fold(0u8, |m, &v| m.max(v)) > threshold {
                    // The edge band (outside `inner`) has the higher threshold.
                    let inner =
                        (spans.inner.0.saturating_sub(start) as usize, spans.inner.1.saturating_sub(start) as usize);
                    let timing =
                        spans.timing.0.saturating_sub(start) as usize..spans.timing.1.saturating_sub(start) as usize;
                    let foreground = |x: usize| {
                        !timing.contains(&x)
                            && row[x] > if x >= inner.0 && x < inner.1 { threshold } else { edge_threshold }
                    };
                    let mut x = 0;
                    while x < row.len() {
                        if !foreground(x) {
                            x += 1;
                            continue;
                        }
                        let run_start = x;
                        while x < row.len() && foreground(x) {
                            x += 1;
                        }
                        let (x0, x1) = (start + run_start as u32, start + x as u32);
                        let len = (x1 - x0) as u64;
                        parent.push(runs.len() as u32);
                        runs.push(Run {
                            x0,
                            x1,
                            area: len as u32,
                            sum_x: len * (x0 as u64 + x1 as u64 - 1) / 2,
                            sum_y: len * y as u64,
                        });
                    }
                }
            }

            // Merge with runs in the previous row that overlap or touch diagonally. Both rows are sorted by x.
            let row_runs = first_run..runs.len();
            let mut j = prev_row.start;
            for i in row_runs.clone() {
                let (x0, x1) = (runs[i].x0, runs[i].x1);
                while j < prev_row.end && runs[j].x1 < x0 {
                    j += 1;
                }
                let mut k = j;
                while k < prev_row.end && runs[k].x0 <= x1 {
                    union(parent, k, i);
                    k += 1;
                }
            }
            prev_row = row_runs;
        }

        // Add each run's totals into its region's root (roots come first, as unions keep the lower index).
        for i in 0..runs.len() {
            let root = find(parent, i);
            if root != i {
                let run = runs[i];
                let r = &mut runs[root];
                r.area += run.area;
                r.sum_x += run.sum_x;
                r.sum_y += run.sum_y;
            }
        }

        let max_area = detection.max_area.unwrap_or(u32::MAX);
        self.blobs.clear();
        self.blobs.extend(
            (0..runs.len())
                .filter(|&i| parent[i] as usize == i)
                .map(|i| runs[i])
                .filter(|r| r.area >= detection.min_area && r.area <= max_area)
                .map(|r| Blob {
                    x: (r.sum_x as f64 / r.area as f64) as f32,
                    y: (r.sum_y as f64 / r.area as f64) as f32,
                    area: r.area,
                }),
        );
        self.blobs.sort_by_key(|b| std::cmp::Reverse(b.area));
        &self.blobs
    }
}

fn find(parent: &mut [u32], mut i: usize) -> usize {
    while parent[i] as usize != i {
        parent[i] = parent[parent[i] as usize]; // path halving
        i = parent[i] as usize;
    }
    i
}

fn union(parent: &mut [u32], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        let (lo, hi) = (ra.min(rb), ra.max(rb));
        parent[hi] = lo as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard_components::tracking::{EdgeThreshold, TimingLed};

    /// A grey frame, for tests.
    struct Image {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    }

    impl Image {
        fn new(width: u32, height: u32, value: u8) -> Self {
            Image { width, height, pixels: vec![value; (width * height) as usize] }
        }

        fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, value: u8) {
            for row in y..y + h {
                for column in x..x + w {
                    self.pixels[(row * self.width + column) as usize] = value;
                }
            }
        }

        fn process(&self, detection: &Detection) -> Vec<Blob> {
            Pipeline::default().process(&self.pixels, self.width, self.height, detection).to_vec()
        }
    }

    fn scene() -> Image {
        let mut image = Image::new(100, 80, 10);
        image.fill(10, 10, 4, 4, 255); // area 16, centre (11.5, 11.5)
        image.fill(60, 40, 6, 5, 255); // area 30, centre (62.5, 42)
        image.fill(90, 70, 1, 2, 255); // area 2, below min_area
        image
    }

    #[test]
    fn finds_blobs_largest_first() {
        let blobs = scene().process(&Detection::default());
        assert_eq!(blobs, vec![Blob { x: 62.5, y: 42.0, area: 30 }, Blob { x: 11.5, y: 11.5, area: 16 }]);
    }

    #[test]
    fn crop_excludes_outside_pixels() {
        let detection = Detection {
            crop: Crop::Rectangle { center: [20.0, 20.0], width: 40.0, height: 40.0, angle: 0.0 },
            ..Default::default()
        };
        assert_eq!(scene().process(&detection), vec![Blob { x: 11.5, y: 11.5, area: 16 }]);
    }

    /// A straightforward implementation (test every pixel, flood fill each region), as a reference.
    fn reference(image: &Image, detection: &Detection) -> Vec<Blob> {
        let (w, h) = (image.width as i64, image.height as i64);
        let inset = detection.crop.inset(detection.edge_width());
        let banded = detection.edge_width() > 0.0;
        let foreground = |x: i64, y: i64| {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let in_band = banded && !inset.is_some_and(|i| i.contains(p));
            let threshold = if in_band { detection.edge_threshold() } else { detection.threshold };
            detection.crop.contains(p) && image.pixels[(y * w + x) as usize] > threshold
        };
        let mut seen = vec![false; (w * h) as usize];
        let mut blobs = Vec::new();
        for start in 0..w * h {
            if seen[start as usize] || !foreground(start % w, start / w) {
                continue;
            }
            let (mut area, mut sum_x, mut sum_y) = (0u32, 0u64, 0u64);
            let mut stack = vec![start];
            seen[start as usize] = true;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                area += 1;
                sum_x += x as u64;
                sum_y += y as u64;
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    let n = ny * w + nx;
                    if nx >= 0 && ny >= 0 && nx < w && ny < h && !seen[n as usize] && foreground(nx, ny) {
                        seen[n as usize] = true;
                        stack.push(n);
                    }
                }
            }
            let max_area = detection.max_area.unwrap_or(u32::MAX);
            if area >= detection.min_area && area <= max_area {
                let centre = |sum: u64| (sum as f64 / area as f64) as f32;
                blobs.push(Blob { x: centre(sum_x), y: centre(sum_y), area });
            }
        }
        blobs
    }

    fn sorted(mut blobs: Vec<Blob>) -> Vec<Blob> {
        blobs.sort_by(|a, b| (b.area, a.x, a.y).partial_cmp(&(a.area, b.x, b.y)).unwrap());
        blobs
    }

    #[test]
    fn matches_reference_on_random_images() {
        let mut seed = 7u64;
        let mut rand = move |n: u32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) % n as u64) as u32
        };
        for case in 0..500 {
            let (w, h) = (20 + rand(60), 20 + rand(60));
            // Sparse to dense foreground, so regions range from specks to large tangled shapes.
            let density = rand(100);
            let mut image = Image::new(w, h, 50);
            for p in image.pixels.iter_mut() {
                if rand(100) < density {
                    *p = [120, 200][rand(2) as usize];
                }
            }
            let center = [rand(w) as f32, rand(h) as f32];
            let crop = match case % 4 {
                0 => Crop::None,
                1 => Crop::Circle { center, radius: 5.0 + rand(40) as f32 },
                2 => Crop::Square { center, size: 5.0 + rand(60) as f32, angle: rand(628) as f32 / 100.0 },
                _ => Crop::Rectangle {
                    center,
                    width: 5.0 + rand(60) as f32,
                    height: 5.0 + rand(60) as f32,
                    angle: rand(628) as f32 / 100.0,
                },
            };
            // Half the cases have an edge band with threshold 150, so values of 120 and 200 land on different
            // sides of it (and both above the main threshold of 100).
            let edge = EdgeThreshold { enabled: case % 8 >= 4, width: rand(15) as f32, threshold: 150 };
            let detection =
                Detection { threshold: 100, min_area: rand(3), crop, edge, max_area: None, ..Default::default() };
            assert_eq!(
                sorted(image.process(&detection)),
                sorted(reference(&image, &detection)),
                "case {case}: {w}x{h}, density {density}%, {crop:?}"
            );
        }
    }

    #[test]
    fn edge_band_suppresses_reflections_near_the_wall() {
        let mut image = Image::new(200, 200, 10);
        image.fill(30, 98, 4, 4, 220); // near the left edge
        image.fill(98, 98, 4, 4, 220); // in the middle
        let crop = Crop::Square { center: [100.0, 100.0], size: 180.0, angle: 0.0 }; // left edge at x=10
        let mut detection = Detection { crop, ..Default::default() };
        assert_eq!(image.process(&detection).len(), 2);

        // Within 30 px of the boundary the threshold is 230, so the 220 LED there doesn't count.
        detection.edge = EdgeThreshold { enabled: true, width: 30.0, threshold: 230 };
        assert_eq!(image.process(&detection), vec![Blob { x: 99.5, y: 99.5, area: 16 }]);
    }

    #[test]
    fn the_timing_spot_is_left_out_and_read_on_its_own_terms() {
        let mut image = Image::new(200, 200, 10);
        image.fill(46, 46, 8, 8, 255); // the timing LED, outside the crop
        image.fill(146, 146, 8, 8, 255); // the animal's
        image.fill(95, 95, 10, 10, 255); // something else, for the spot to go over
        let crop = Crop::Circle { center: [130.0, 130.0], radius: 50.0 };
        let timing_led = TimingLed { enabled: true, center: [50.0, 50.0], radius: 10.0, ..Default::default() };
        let mut detection = Detection { crop, timing_led, ..Default::default() };
        let on = |d: &Detection| timing_led_on(&image.pixels, image.width, image.height, d);

        assert_eq!(on(&detection), Some(true), "whatever the crop");
        detection.timing_led.center = [20.0, 20.0];
        assert_eq!(on(&detection), Some(false), "nothing's there");
        detection.timing_led.center = [50.0, 50.0];
        detection.timing_led.enabled = false;
        assert_eq!(on(&detection), None, "there's none");

        // Its threshold and area are the others', unless it has its own.
        detection.timing_led.enabled = true;
        detection.threshold = 255;
        assert_eq!(on(&detection), Some(false));
        (detection.timing_led.separate, detection.timing_led.threshold) = (true, 200);
        assert_eq!(on(&detection), Some(true));
        detection.timing_led.min_area = 10_000;
        assert_eq!(on(&detection), Some(false));

        // The others don't see what's in its spot.
        let mut detection = Detection::default();
        let all = image.process(&detection).len();
        detection.timing_led = TimingLed { enabled: true, center: [100.0, 100.0], radius: 14.0, ..Default::default() };
        assert_eq!(image.process(&detection).len(), all - 1);
    }

    #[test]
    fn row_spans_match_per_pixel_test() {
        let (w, h) = (720u32, 540u32);
        let crops = [
            Crop::Circle { center: [350.3, 256.8], radius: 215.7 },
            Crop::Circle { center: [10.0, 10.0], radius: 3.2 },
            Crop::Square { center: [360.0, 270.0], size: 450.0, angle: 0.0 },
            Crop::Square { center: [359.5, 270.5], size: 388.7, angle: std::f32::consts::FRAC_PI_4 },
            Crop::Rectangle { center: [700.0, 50.0], width: 300.0, height: 45.0, angle: 1.3 },
            Crop::Rectangle { center: [360.0, 270.0], width: 1000.0, height: 200.0, angle: -0.05 },
        ];
        for crop in crops {
            for y in 0..h {
                let inside: Vec<u32> = (0..w).filter(|&x| crop.contains([x as f32 + 0.5, y as f32 + 0.5])).collect();
                let expected = match (inside.first(), inside.last()) {
                    (Some(&a), Some(&b)) => (a, b + 1),
                    _ => (0, 0),
                };
                assert_eq!(row_span(&crop, y, w), expected, "{crop:?}, row {y}");
            }
        }
    }
}
