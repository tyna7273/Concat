// =============================================================================
// concat.mesh-warp — GPU Mesh / Grid Warp Effect
// =============================================================================
// Deforms the video by displacing UV coordinates through a smooth bilinear
// grid. Each cell of a cols×rows grid contributes a displacement vector,
// shaped by a Gaussian falloff centred at (pin_x, pin_y).
//
// mode 0 = Bulge   — pushes pixels outward from the centre (magnify)
// mode 1 = Pinch   — pulls pixels inward toward the centre (shrink)
// mode 2 = Ripple  — sine-wave displacement radiating from centre
// =============================================================================

struct Params {
    strength : f32,
    cols     : f32,
    rows     : f32,
    pin_x    : f32,
    pin_y    : f32,
    radius   : f32,
    mode     : f32,
}

// ---------------------------------------------------------------------------
// Smooth noise helpers — used for per-cell seeding so the grid is not flat
// ---------------------------------------------------------------------------

fn hash2(p: vec2<f32>) -> f32 {
    let k = vec2<f32>(0.3183099, 0.3678794);
    let x = p * k + k.yx;
    return fract(16.0 * k.x * fract(x.x * x.y * (x.x + x.y)));
}

// ---------------------------------------------------------------------------
// Bilinear interpolation across the warp grid
// ---------------------------------------------------------------------------
// Each grid vertex carries a displacement offset derived from the warp
// parameters. We resolve the displacement at `uv` by finding which cell
// it falls in, computing the four corner displacements, and blending them.

fn corner_disp(cell: vec2<f32>, centre: vec2<f32>, radius: f32, strength: f32, mode: f32) -> vec2<f32> {
    // Normalised position of this grid vertex in [0,1]²
    let cols = max(params.cols, 2.0);
    let rows = max(params.rows, 2.0);
    let vert_uv = cell / vec2<f32>(cols - 1.0, rows - 1.0);

    let delta  = vert_uv - centre;
    let dist   = length(delta);
    let r      = max(radius, 0.001);

    // Gaussian falloff — peak 1 at centre, drops to ~0 at radius
    let gauss  = exp(-4.0 * (dist / r) * (dist / r));

    let dir    = select(normalize(delta), vec2<f32>(0.0), dist < 0.0001);
    let amp    = strength * gauss;

    if mode < 0.5 {
        // Bulge: push outward
        return dir * amp;
    } else if mode < 1.5 {
        // Pinch: pull inward
        return -dir * amp;
    } else {
        // Ripple: sine wave displacement perpendicular to radial dir
        let wave   = sin(dist / r * 6.2832 * 2.0) * amp;
        let perp   = vec2<f32>(-dir.y, dir.x);
        return perp * wave;
    }
}

fn mesh_warp_uv(uv: vec2<f32>) -> vec2<f32> {
    let cols     = max(params.cols, 2.0);
    let rows     = max(params.rows, 2.0);
    let centre   = vec2<f32>(params.pin_x, params.pin_y) / 100.0;
    let radius   = params.radius / 100.0;
    let strength = params.strength / 100.0 * 0.25; // scale to safe UV range
    let mode     = params.mode;

    // Which cell does `uv` fall into?
    let grid_pos = uv * vec2<f32>(cols - 1.0, rows - 1.0);
    let cell_f   = floor(grid_pos);
    let frac     = grid_pos - cell_f;

    // Four corner cells (clamped to grid)
    let c00 = clamp(cell_f,                          vec2<f32>(0.0), vec2<f32>(cols - 1.0, rows - 1.0));
    let c10 = clamp(cell_f + vec2<f32>(1.0, 0.0),   vec2<f32>(0.0), vec2<f32>(cols - 1.0, rows - 1.0));
    let c01 = clamp(cell_f + vec2<f32>(0.0, 1.0),   vec2<f32>(0.0), vec2<f32>(cols - 1.0, rows - 1.0));
    let c11 = clamp(cell_f + vec2<f32>(1.0, 1.0),   vec2<f32>(0.0), vec2<f32>(cols - 1.0, rows - 1.0));

    // Displacement at each corner vertex
    let d00 = corner_disp(c00, centre, radius, strength, mode);
    let d10 = corner_disp(c10, centre, radius, strength, mode);
    let d01 = corner_disp(c01, centre, radius, strength, mode);
    let d11 = corner_disp(c11, centre, radius, strength, mode);

    // Smooth bilinear interpolation (using smoothstep for C1 continuity)
    let sx  = smoothstep(0.0, 1.0, frac.x);
    let sy  = smoothstep(0.0, 1.0, frac.y);

    let disp = mix(mix(d00, d10, sx), mix(d01, d11, sx), sy);

    return uv + disp;
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let warped_uv = mesh_warp_uv(uv);
    return sample(warped_uv);
}
