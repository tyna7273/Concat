// =============================================================================
// concat.mask-path — Pen Tool / Custom Polygon Mask Effect
// =============================================================================
// Evaluates the signed distance field (SDF) of an arbitrary 3 to 8 point
// polygon mask with optional rounded corners, feathering, and invert toggle.
// =============================================================================

struct Params {
    points    : f32,
    feather   : f32,
    invert    : f32,
    opacity   : f32,
    roundness : f32,
    p1_x      : f32,
    p1_y      : f32,
    p2_x      : f32,
    p2_y      : f32,
    p3_x      : f32,
    p3_y      : f32,
    p4_x      : f32,
    p4_y      : f32,
    p5_x      : f32,
    p5_y      : f32,
    p6_x      : f32,
    p6_y      : f32,
    p7_x      : f32,
    p7_y      : f32,
    p8_x      : f32,
    p8_y      : f32,
}

fn get_point(index: i32) -> vec2<f32> {
    if (index == 0) { return vec2<f32>(params.p1_x, params.p1_y) / 100.0; }
    if (index == 1) { return vec2<f32>(params.p2_x, params.p2_y) / 100.0; }
    if (index == 2) { return vec2<f32>(params.p3_x, params.p3_y) / 100.0; }
    if (index == 3) { return vec2<f32>(params.p4_x, params.p4_y) / 100.0; }
    if (index == 4) { return vec2<f32>(params.p5_x, params.p5_y) / 100.0; }
    if (index == 5) { return vec2<f32>(params.p6_x, params.p6_y) / 100.0; }
    if (index == 6) { return vec2<f32>(params.p7_x, params.p7_y) / 100.0; }
    return vec2<f32>(params.p8_x, params.p8_y) / 100.0;
}

// Distance from point p to line segment (a, b)
fn dist_to_segment_sq(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let len_sq = max(dot(ba, ba), 1e-8);
    let h = clamp(dot(pa, ba) / len_sq, 0.0, 1.0);
    let diff = pa - ba * h;
    return dot(diff, diff);
}

// Signed distance to arbitrary n-sided polygon
fn sd_polygon(p: vec2<f32>, n: i32) -> f32 {
    var min_d_sq = 1e8;
    var inside = false;

    for (var i = 0; i < 8; i++) {
        if (i >= n) {
            break;
        }
        let j = (i + 1) % n;
        let vi = get_point(i);
        let vj = get_point(j);

        min_d_sq = min(min_d_sq, dist_to_segment_sq(p, vi, vj));

        // Ray casting algorithm for winding test
        let cond1 = (vi.y > p.y) != (vj.y > p.y);
        let edge_dx = vj.x - vi.x;
        let edge_dy = vj.y - vi.y;
        let cond2 = p.x < (edge_dx * (p.y - vi.y) / max(abs(edge_dy), 1e-6) + vi.x);

        if (cond1 && cond2) {
            inside = !inside;
        }
    }

    let d = sqrt(max(min_d_sq, 0.0));
    return select(d, -d, inside);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = sample(uv);
    let n = clamp(i32(round(params.points)), 3, 8);
    let radius = params.roundness / 100.0 * 0.1;

    let dist = sd_polygon(uv, n) - radius;
    let feather = max(params.feather / 100.0 * 0.25, 0.002);

    // Coverage: 1 inside, 0 outside, smoothly interpolated along feather band
    var coverage = 1.0 - smoothstep(-feather * 0.5, feather * 0.5, dist);

    if (params.invert > 0.5) {
        coverage = 1.0 - coverage;
    }

    let alpha_scale = mix(1.0, coverage, params.opacity / 100.0);
    return vec4<f32>(c.rgb, c.a * alpha_scale);
}
