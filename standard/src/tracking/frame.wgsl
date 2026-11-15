// A camera frame, shaded as the Tracker View shows it (see `frame_gpu.rs`, and `colour_image` in `tracker_view.rs`,
// which this matches): grey, with above-threshold pixels in the crop highlighted, the outside of the crop black, and
// the timing LED's spot with its own threshold. Shrunk, each screen pixel is the average of the frame's pixels under
// it, or highlighted if any is; enlarged, it's between the four nearest, as a linearly filtered texture would be.

// A shape, as `Crop::contains` tests it: `a` is its centre and half extents (a circle's radius in both), `b` its
// rotation's cosine and sine, and its kind: 0 everything, 1 nothing, 2 a circle, 3 a box.
struct Shape {
    a: vec4<f32>,
    b: vec4<f32>,
}

struct Params {
    // Where it's drawn, in physical pixels: left, top, width, height.
    rect: vec4<f32>,
    // The threshold, the edge band's, and the timing LED's.
    thresholds: vec4<u32>,
    // x: bits saying what's shown (see `FLAG_*`).
    flags: vec4<u32>,
    crop: Shape,
    // Inside the crop and outside its edge band.
    inner: Shape,
    timing: Shape,
}

const FLAG_DETECTION: u32 = 1u; // there are detection settings
const FLAG_CROP_SHOWN: u32 = 2u; // the whole frame's shown, not just the crop
const FLAG_DETECTION_SHOWN: u32 = 4u; // above-threshold pixels are highlighted
const FLAG_TIMING_SHOWN: u32 = 8u; // the timing LED's spot is shown
const FLAG_CROPPED: u32 = 16u; // there's a crop
const FLAG_LINEAR_OUTPUT: u32 = 32u; // the target's sRGB, so expects linear colours

const HIGHLIGHT: vec3<f32> = vec3<f32>(255.0, 70.0, 70.0);

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var frame: texture_2d<u32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // A triangle covering the viewport.
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn has(flag: u32) -> bool {
    return (params.flags.x & flag) != 0u;
}

fn inside(shape: Shape, p: vec2<f32>) -> bool {
    let kind = u32(shape.b.z);
    if kind == 0u {
        return true;
    }
    if kind == 1u {
        return false;
    }
    let d = p - shape.a.xy;
    let c = shape.b.x;
    let s = shape.b.y;
    let local = vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c);
    if kind == 2u {
        return local.x * local.x + local.y * local.y <= shape.a.z * shape.a.z;
    }
    return abs(local.x) <= shape.a.z && abs(local.y) <= shape.a.w;
}

// Frame pixel `at`: its grey (0 for black), and whether it's highlighted.
struct Shaded {
    grey: u32,
    lit: bool,
}

fn shade(at: vec2<i32>) -> Shaded {
    let value = textureLoad(frame, at, 0).r;
    if !has(FLAG_DETECTION) {
        return Shaded(value, false);
    }
    let p = vec2<f32>(at) + 0.5;
    let in_crop = inside(params.crop, p);
    let in_band = !inside(params.inner, p);
    let in_timing = inside(params.timing, p);
    if in_timing && has(FLAG_TIMING_SHOWN) {
        return Shaded(value, value > params.thresholds.z);
    }
    if !in_crop && !has(FLAG_CROP_SHOWN) && has(FLAG_CROPPED) {
        return Shaded(0u, false);
    }
    if in_crop && !in_timing && has(FLAG_DETECTION_SHOWN) {
        let threshold = select(params.thresholds.x, params.thresholds.y, in_band);
        return Shaded(value, value > threshold);
    }
    return Shaded(value, false);
}

// The pixel `d` from `i`, kept within the frame.
fn near(i: vec2<i32>, d: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return clamp(i + d, vec2<i32>(0), size - 1);
}

// Its colour, 0–255 a channel.
fn colour(shaded: Shaded) -> vec3<f32> {
    return select(vec3<f32>(f32(shaded.grey)), HIGHLIGHT, shaded.lit);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(frame));
    let scale = vec2<f32>(size) / params.rect.zw;
    // Where this screen pixel's centre is, in the frame's pixels.
    let p = (position.xy - params.rect.xy) * scale;
    var rgb: vec3<f32>;
    if scale.x > 1.0 || scale.y > 1.0 {
        // Shrunk: the frame's pixels under this one, at most 32 each way (beyond that, evenly spaced).
        let first = clamp(vec2<i32>(floor(p - scale * 0.5)), vec2<i32>(0), size - 1);
        let end = clamp(vec2<i32>(ceil(p + scale * 0.5)), first + 1, size);
        let step = max((end - first + 31) / 32, vec2<i32>(1));
        var sum = 0u;
        var count = 0u;
        var lit = false;
        for (var y = first.y; y < end.y; y += step.y) {
            for (var x = first.x; x < end.x; x += step.x) {
                let shaded = shade(vec2<i32>(x, y));
                sum += shaded.grey;
                count += 1u;
                lit = lit || shaded.lit;
            }
        }
        rgb = select(vec3<f32>(f32(sum / count)), HIGHLIGHT, lit);
    } else {
        // Enlarged: between the four nearest pixels' colours.
        let q = p - 0.5;
        let corner = floor(q);
        let f = q - corner;
        let i = vec2<i32>(corner);
        let top_left = colour(shade(near(i, vec2<i32>(0, 0), size)));
        let top_right = colour(shade(near(i, vec2<i32>(1, 0), size)));
        let bottom_left = colour(shade(near(i, vec2<i32>(0, 1), size)));
        let bottom_right = colour(shade(near(i, vec2<i32>(1, 1), size)));
        rgb = mix(mix(top_left, top_right, f.x), mix(bottom_left, bottom_right, f.x), f.y);
    }
    rgb = rgb / 255.0;
    if has(FLAG_LINEAR_OUTPUT) {
        rgb = select(pow((rgb + 0.055) / 1.055, vec3<f32>(2.4)), rgb / 12.92, rgb <= vec3<f32>(0.04045));
    }
    return vec4<f32>(rgb, 1.0);
}
