// Brute-force O(n²) gravity: every body pulls on every other body.
//
// Classic tiled n-body kernel: each workgroup walks the body list in tiles of 256, staging
// each tile in workgroup memory so every body is read from global memory once per workgroup
// rather than once per thread.
//
// Softened (Plummer) gravity: a_i = G · Σ_j m_j · d / (|d|² + ε²)^(3/2), with d = x_j − x_i.
// The j = i term has d = 0 and contributes nothing, so it needs no special case.

import package::gpu::shaders::types::SimParams;

const TILE: u32 = 256u;

@group(0) @binding(0) var<uniform> params: SimParams;
// (x, y, mass, bitcast<f32>(particle slot)); first `body_count` entries are valid.
@group(0) @binding(1) var<storage, read> bodies: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> body_count: u32;
// Acceleration per particle slot.
@group(0) @binding(3) var<storage, read_write> accel: array<vec2<f32>>;

var<workgroup> tile: array<vec4<f32>, TILE>;
var<workgroup> shared_count: u32;

@compute @workgroup_size(256)
fn gravity(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
    @builtin(workgroup_id) group: vec3<u32>,
) {
    // The loop below has barriers, so its bounds must be uniform across the workgroup:
    // workgroupUniformLoad gives every thread the same count, provably.
    if local == 0u {
        shared_count = min(body_count, params.gravity_max_bodies);
    }
    let n = workgroupUniformLoad(&shared_count);
    if group.x * TILE >= n {
        return;
    }

    let i = global_id.x;
    let is_body = i < n;
    var me = vec4(0.0);
    if is_body {
        me = bodies[i];
    }

    var a = vec2(0.0);
    for (var base = 0u; base < n; base += TILE) {
        // Every thread loads one body of the tile; past the end, mass 0 pads it out.
        let j = base + local;
        var b = vec4(0.0);
        if j < n {
            b = bodies[j];
        }
        tile[local] = b;
        workgroupBarrier();

        for (var k = 0u; k < TILE; k++) {
            let other = tile[k];
            let d = other.xy - me.xy;
            let r2 = dot(d, d) + params.gravity_softening_sq;
            a += d * (other.z * inverseSqrt(r2 * r2 * r2));
        }
        workgroupBarrier();
    }

    if is_body {
        accel[bitcast<u32>(me.w)] = params.gravity_strength * a;
    }
}
