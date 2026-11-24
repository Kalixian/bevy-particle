// Lists the live slots, in slot order, so that drawing only costs what's alive. (Appending
// through an atomic counter would be simpler, but its order changes from frame to frame, which
// makes overlapping alpha-blended particles flicker.) Entry points, in dispatch order:
//
//   mark         one bit per slot (256 slots per chunk = 8 words), and the live count per chunk
//   scan_blocks  } exclusive prefix sum of the chunk counts; also writes the draw arguments
//   scan_sums    }
//   write        each live slot's index, at its rank

import package::gpu::shaders::types::{Particle, SimParams, is_alive};
import package::gpu::shaders::scan::scan_exclusive;

const WG: u32 = 256u;
const VERTICES_PER_PARTICLE: u32 = 6u;

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var<storage, read_write> masks: array<u32>;
// One per chunk, then one sum per 256 chunks.
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;
@group(0) @binding(4) var<storage, read_write> live: array<u32>;
// (vertex count, instance count, first vertex, first instance)
@group(0) @binding(5) var<storage, read_write> draw_args: array<u32, 4>;

var<workgroup> wg_masks: array<atomic<u32>, 8>;

fn chunks() -> u32 {
    return (params.capacity + WG - 1u) / WG;
}

@compute @workgroup_size(256)
fn mark(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let slot = wid.x * WG + lid;
    if slot < params.capacity && is_alive(particles[slot]) {
        atomicOr(&wg_masks[lid / 32u], 1u << (lid % 32u));
    }
    workgroupBarrier();
    if lid < 8u {
        masks[wid.x * 8u + lid] = atomicLoad(&wg_masks[lid]);
    }
    if lid == 0u {
        var count = 0u;
        for (var w = 0u; w < 8u; w++) {
            count += countOneBits(atomicLoad(&wg_masks[w]));
        }
        counts[wid.x] = count;
    }
}

@compute @workgroup_size(256)
fn scan_blocks(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let len = chunks();
    let i = wid.x * WG + lid;
    var v = 0u;
    if i < len {
        v = counts[i];
    }
    let r = scan_exclusive(lid, v);
    if i < len {
        counts[i] = r.x;
    }
    if lid == 0u {
        counts[len + wid.x] = r.y;
    }
}

// At most 256 blocks: one workgroup.
@compute @workgroup_size(256)
fn scan_sums(@builtin(local_invocation_index) lid: u32) {
    let len = chunks();
    let blocks = (len + WG - 1u) / WG;
    var v = 0u;
    if lid < blocks {
        v = counts[len + lid];
    }
    let r = scan_exclusive(lid, v);
    if lid < blocks {
        counts[len + lid] = r.x;
    }
    if lid == 0u {
        draw_args = array<u32, 4>(VERTICES_PER_PARTICLE, r.y, 0u, 0u);
    }
}

@compute @workgroup_size(256)
fn write(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let chunk_masks = wid.x * 8u;
    let word = lid / 32u;
    let mask = masks[chunk_masks + word];
    let bit = 1u << (lid % 32u);
    if (mask & bit) == 0u {
        return;
    }
    var rank = countOneBits(mask & (bit - 1u));
    for (var w = 0u; w < word; w++) {
        rank += countOneBits(masks[chunk_masks + w]);
    }
    live[counts[wid.x] + counts[chunks() + wid.x / WG] + rank] = wid.x * WG + lid;
}
