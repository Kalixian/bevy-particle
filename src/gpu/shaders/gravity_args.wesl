// Sizes the gravity dispatches from the body count that `compact` wrote.

import package::gpu::shaders::types::SimParams;

const WG: u32 = 256u;

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var<storage, read> body_count: u32;
// Two sets of workgroup counts (x, y, z, unused): one thread per body, and one thread per radix
// histogram entry (16 digits per tile of 256 bodies). Offsets must match src/gpu/gravity.rs.
@group(0) @binding(2) var<storage, read_write> gravity_args: array<vec4<u32>, 2>;

@compute @workgroup_size(1)
fn size_gravity() {
    let tiles = (min(body_count, params.gravity_max_bodies) + WG - 1u) / WG;
    gravity_args[0] = vec4(tiles, 1u, 1u, 0u);
    gravity_args[1] = vec4((16u * tiles + WG - 1u) / WG, 1u, 1u, 0u);
}
