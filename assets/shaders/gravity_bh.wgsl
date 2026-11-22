// Barnes-Hut gravity, fully on the GPU. Entry points, in dispatch order (see src/gpu/gravity.rs):
//
//   bounds_pass        bounding box of all bodies (atomic min/max)
//   morton             32-bit Morton code per body (16 bits per axis) -> keys/vals[0..n]
//   radix_histogram  â”
//   radix_scan_blocksâ”‚ x8: stable LSD radix sort, 4 bits per pass, ping-ponging between
//   radix_scan_sums  â”‚     keys/vals[0..M] and keys/vals[M..2M]; ends back in [0..M]
//   radix_scatter    â”˜
//   build_tree         Karras 2012 radix tree over the sorted keys: n leaves, n-1 internal nodes
//   summarize          bottom-up mass and center of mass per internal node
//   force              each body walks the tree; nodes with size/distance < θ count as one mass
//
// The body count n is only known on the GPU (written by `compact`), so every pass is
// dispatched for the maximum M and threads past n do nothing.

// Must match `SimParams` in src/space.rs.
struct SimParams {
    dt: f32,
    capacity: u32,
    spawn_total: u32,
    emitter_count: u32,
    frame: u32,
    modifier_offset: u32,
    modifier_count: u32,
    gravity_enabled: u32,
    gravity_strength: f32,
    gravity_softening_sq: f32,
    gravity_method: u32,
    gravity_max_bodies: u32,
    gravity_theta_sq: f32,
}

// Must match `RadixPassParams` in src/gpu/gravity.rs. Bound with a dynamic offset per pass.
struct RadixPass {
    shift: u32,
    src: u32,
    dst: u32,
}

// Internal tree node. Children with the LEAF bit set are leaves: indices into the sorted
// body order (keys/vals[0..n]). Otherwise they're internal node indices.
struct Node {
    left: u32,
    right: u32,
    parent: u32,
    // Side length of the node's Morton cell (an upper bound on its spatial extent).
    size: f32,
}

const WG: u32 = 256u;
const LEAF: u32 = 0x80000000u;
const NONE: u32 = 0xFFFFFFFFu;
const STACK_SIZE: u32 = 32u;
// Workgroup size of `force`; must match FORCE_WORKGROUP_SIZE in src/gpu/gravity.rs.
const FORCE_WG: u32 = 256u;

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var<uniform> radix: RadixPass;
// (x, y, mass, bitcast<f32>(particle slot)), compacted; first `body_count` are valid.
@group(0) @binding(2) var<storage, read_write> bodies: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> body_count: atomic<u32>;
// Acceleration per particle slot.
@group(0) @binding(4) var<storage, read_write> accel: array<vec2<f32>>;
// Order-preserving encoded floats, all reduced with atomicMax (so a zeroed buffer is the
// identity): [~min_x, ~min_y, max_x, max_y]. Cleared to 0 every frame.
@group(0) @binding(5) var<storage, read_write> bounds: array<atomic<u32>, 4>;
// Sort keys (Morton codes) and values (body indices), two halves of M for ping-ponging.
@group(0) @binding(6) var<storage, read_write> keys: array<u32>;
@group(0) @binding(7) var<storage, read_write> vals: array<u32>;
// [digit * tiles + tile] counts/offsets (16 × tiles), then one sum per 256-entry block.
@group(0) @binding(8) var<storage, read_write> hist: array<u32>;
@group(0) @binding(9) var<storage, read_write> nodes: array<Node>;
@group(0) @binding(10) var<storage, read_write> leaf_parent: array<u32>;
// Per internal node: (mass, com_x, com_y) as f32 bits. Atomic so that `summarize` threads
// see each other's results; see the note there.
@group(0) @binding(11) var<storage, read_write> node_data: array<atomic<u32>>;
// Per internal node: how many children have finished. Cleared to 0 every frame.
@group(0) @binding(12) var<storage, read_write> visits: array<atomic<u32>>;
// Per internal node, written once by `summarize` with plain stores: (com_x, com_y, mass, size).
// `force` reads only this (one 16-byte load per node, cacheable), never the atomics.
@group(0) @binding(13) var<storage, read_write> packed_nodes: array<vec4<f32>>;
// bodies[] in sorted (Morton) order, written by `build_tree`, so tree walks read neighboring
// bodies from neighboring memory instead of going through `vals`.
@group(0) @binding(14) var<storage, read_write> sorted_bodies: array<vec4<f32>>;

var<workgroup> wg_n: u32;
var<workgroup> wg_bounds: array<atomic<u32>, 4>;
var<workgroup> wg_hist: array<atomic<u32>, 16>;
var<workgroup> wg_scan: array<u32, WG>;
var<workgroup> wg_masks: array<atomic<u32>, 128>; // 16 digits × 8 words of 32 lanes

// --- shared helpers ---------------------------------------------------------------------

fn body_total() -> u32 {
    return min(atomicLoad(&body_count), params.gravity_max_bodies);
}

// Body count, provably uniform across the workgroup (needed before barriers). Includes a
// workgroup barrier.
fn uniform_body_total(lid: u32) -> u32 {
    if lid == 0u {
        wg_n = body_total();
    }
    return workgroupUniformLoad(&wg_n);
}

// Number of 256-body tiles for the maximum body count.
fn tiles() -> u32 {
    return (params.gravity_max_bodies + WG - 1u) / WG;
}

// Maps f32 to u32 so that unsigned order matches float order.
fn ordered(f: f32) -> u32 {
    let u = bitcast<u32>(f);
    return select(u | 0x80000000u, ~u, (u & 0x80000000u) != 0u);
}

fn unordered(u: u32) -> f32 {
    return bitcast<f32>(select(~u, u & 0x7FFFFFFFu, (u & 0x80000000u) != 0u));
}

// (origin.x, origin.y, side) of the square that the Morton grid spans.
fn domain() -> vec3<f32> {
    let lo = vec2(unordered(~atomicLoad(&bounds[0])), unordered(~atomicLoad(&bounds[1])));
    let hi = vec2(unordered(atomicLoad(&bounds[2])), unordered(atomicLoad(&bounds[3])));
    let side = max(max(hi.x - lo.x, hi.y - lo.y), 1e-3) * 1.0001;
    return vec3(lo, side);
}

// Exclusive prefix sum across the workgroup (Hillis-Steele). Returns (exclusive, total).
fn scan_exclusive(lid: u32, v: u32) -> vec2<u32> {
    wg_scan[lid] = v;
    workgroupBarrier();
    for (var offset = 1u; offset < WG; offset <<= 1u) {
        var t = 0u;
        if lid >= offset {
            t = wg_scan[lid - offset];
        }
        workgroupBarrier();
        wg_scan[lid] += t;
        workgroupBarrier();
    }
    return vec2(wg_scan[lid] - v, wg_scan[WG - 1u]);
}

// --- 1. bounds --------------------------------------------------------------------------

@compute @workgroup_size(256)
fn bounds_pass(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    if lid < 4u {
        atomicStore(&wg_bounds[lid], 0u);
    }
    let n = uniform_body_total(lid);
    if gid.x < n {
        let p = bodies[gid.x].xy;
        atomicMax(&wg_bounds[0], ~ordered(p.x));
        atomicMax(&wg_bounds[1], ~ordered(p.y));
        atomicMax(&wg_bounds[2], ordered(p.x));
        atomicMax(&wg_bounds[3], ordered(p.y));
    }
    workgroupBarrier();
    if lid < 4u {
        let v = atomicLoad(&wg_bounds[lid]);
        if v != 0u {
            atomicMax(&bounds[lid], v);
        }
    }
}

// --- 2. Morton codes --------------------------------------------------------------------

// Spreads the low 16 bits of x to the even bits.
fn spread_bits(v: u32) -> u32 {
    var x = v & 0xFFFFu;
    x = (x | (x << 8u)) & 0x00FF00FFu;
    x = (x | (x << 4u)) & 0x0F0F0F0Fu;
    x = (x | (x << 2u)) & 0x33333333u;
    x = (x | (x << 1u)) & 0x55555555u;
    return x;
}

@compute @workgroup_size(256)
fn morton(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= body_total() {
        return;
    }
    let d = domain();
    let f = clamp((bodies[i].xy - d.xy) / d.z, vec2(0.0), vec2(0.99999));
    let q = vec2<u32>(f * 65536.0);
    keys[i] = spread_bits(q.x) | (spread_bits(q.y) << 1u);
    vals[i] = i;
}

// --- 3. radix sort (one 4-bit digit per pass) -------------------------------------------

// Per-tile digit counts -> hist[digit * tiles + tile].
@compute @workgroup_size(256)
fn radix_histogram(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    if lid < 16u {
        atomicStore(&wg_hist[lid], 0u);
    }
    let n = uniform_body_total(lid);
    let i = wid.x * WG + lid;
    if i < n {
        let digit = (keys[radix.src + i] >> radix.shift) & 15u;
        atomicAdd(&wg_hist[digit], 1u);
    }
    workgroupBarrier();
    if lid < 16u {
        hist[lid * tiles() + wid.x] = atomicLoad(&wg_hist[lid]);
    }
}

// Exclusive scan within each 256-entry block of hist; block totals go after the counts.
@compute @workgroup_size(256)
fn radix_scan_blocks(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let len = 16u * tiles();
    let idx = wid.x * WG + lid;
    var v = 0u;
    if idx < len {
        v = hist[idx];
    }
    let r = scan_exclusive(lid, v);
    if idx < len {
        hist[idx] = r.x;
    }
    if lid == 0u {
        hist[len + wid.x] = r.y;
    }
}

// Exclusive scan of the block totals (at most 256 of them: one workgroup).
@compute @workgroup_size(256)
fn radix_scan_sums(@builtin(local_invocation_index) lid: u32) {
    let len = 16u * tiles();
    let blocks = (len + WG - 1u) / WG;
    var v = 0u;
    if lid < blocks {
        v = hist[len + lid];
    }
    let r = scan_exclusive(lid, v);
    if lid < blocks {
        hist[len + lid] = r.x;
    }
}

// Moves each key to its sorted position. Stable: within a tile, equal digits keep lane
// order (ranked with per-digit lane bitmasks); across tiles, the bin-major scan keeps tile order.
@compute @workgroup_size(256)
fn radix_scatter(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    atomicStore(&wg_masks[lid % 128u], 0u);
    let n = uniform_body_total(lid);
    let i = wid.x * WG + lid;
    let valid = i < n;
    let word = lid / 32u;
    let bit = lid % 32u;

    var key = 0u;
    var val = 0u;
    var digit = 0u;
    if valid {
        key = keys[radix.src + i];
        val = vals[radix.src + i];
        digit = (key >> radix.shift) & 15u;
        atomicOr(&wg_masks[digit * 8u + word], 1u << bit);
    }
    workgroupBarrier();

    if valid {
        var rank = 0u;
        for (var w = 0u; w < word; w++) {
            rank += countOneBits(atomicLoad(&wg_masks[digit * 8u + w]));
        }
        rank += countOneBits(atomicLoad(&wg_masks[digit * 8u + word]) & ((1u << bit) - 1u));

        let h = digit * tiles() + wid.x;
        let position = hist[h] + hist[16u * tiles() + h / WG] + rank;
        keys[radix.dst + position] = key;
        vals[radix.dst + position] = val;
    }
}

// --- 4. tree build (Karras, "Maximizing Parallelism in the Construction of BVHs,
//        Octrees, and k-d Trees", 2012) -----------------------------------------------------

// Length of the common prefix of sorted keys i and j; -1 if j is out of range.
// Equal keys fall back to comparing indices, which makes every key unique.
fn delta(i: u32, j: i32, n: u32) -> i32 {
    if j < 0 || j >= i32(n) {
        return -1;
    }
    let ki = keys[i];
    let kj = keys[u32(j)];
    if ki == kj {
        return 32 + i32(countLeadingZeros(i ^ u32(j)));
    }
    return i32(countLeadingZeros(ki ^ kj));
}

@compute @workgroup_size(256)
fn build_tree(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = body_total();
    let i = gid.x;
    if i < n {
        sorted_bodies[i] = bodies[vals[i]];
    }
    if n < 2u || i >= n - 1u {
        return;
    }
    let ii = i32(i);

    // Direction of this node's range, and its far end j.
    let d = select(-1, 1, delta(i, ii + 1, n) > delta(i, ii - 1, n));
    let delta_min = delta(i, ii - d, n);
    var l_max = 2;
    while delta(i, ii + l_max * d, n) > delta_min {
        l_max *= 2;
    }
    var l = 0;
    for (var t = l_max / 2; t >= 1; t /= 2) {
        if delta(i, ii + (l + t) * d, n) > delta_min {
            l += t;
        }
    }
    let j = ii + l * d;

    // Split position: the last index sharing more than delta_node prefix bits with i.
    let delta_node = delta(i, j, n);
    var s = 0;
    var step = l;
    loop {
        step = (step + 1) / 2;
        if delta(i, ii + (s + step) * d, n) > delta_node {
            s += step;
        }
        if step <= 1 {
            break;
        }
    }
    let gamma = u32(ii + s * d + min(d, 0));

    var left = gamma;
    if u32(min(ii, j)) == gamma {
        left = gamma | LEAF;
        leaf_parent[gamma] = i;
    } else {
        nodes[gamma].parent = i;
    }
    var right = gamma + 1u;
    if u32(max(ii, j)) == gamma + 1u {
        right = (gamma + 1u) | LEAF;
        leaf_parent[gamma + 1u] = i;
    } else {
        nodes[gamma + 1u].parent = i;
    }
    nodes[i].left = left;
    nodes[i].right = right;

    // A common prefix of p bits pins floor(p/2) bits of each axis: the node lies in a square
    // cell of side domain / 2^floor(p/2).
    let levels = u32(min(delta_node, 32)) / 2u;
    nodes[i].size = domain().z / f32(1u << levels);

    if i == 0u {
        nodes[0].parent = NONE;
    }
}

// --- 5. mass summary --------------------------------------------------------------------

// (mass, com_x, com_y) of a child.
fn child_summary(c: u32) -> vec3<f32> {
    if (c & LEAF) != 0u {
        let b = sorted_bodies[c & ~LEAF];
        return vec3(b.z, b.xy);
    }
    return vec3(
        bitcast<f32>(atomicLoad(&node_data[3u * c])),
        bitcast<f32>(atomicLoad(&node_data[3u * c + 1u])),
        bitcast<f32>(atomicLoad(&node_data[3u * c + 2u])),
    );
}

// One thread per leaf walks toward the root. At each node the first child to arrive stops;
// the second (whose sibling is therefore finished) combines both and continues upward.
//
// Note: WGSL has no device-scope memory fence, so this relies on atomics to the same
// buffer becoming visible in issue order, which holds on current desktop GPUs (it's the
// standard CUDA approach minus __threadfence).
@compute @workgroup_size(256)
fn summarize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = body_total();
    let leaf = gid.x;
    if n < 2u || leaf >= n {
        return;
    }
    var node = leaf_parent[leaf];
    loop {
        if atomicAdd(&visits[node], 1u) == 0u {
            return;
        }
        let a = child_summary(nodes[node].left);
        let b = child_summary(nodes[node].right);
        let mass = a.x + b.x;
        var com = (a.yz + b.yz) * 0.5;
        if mass > 0.0 {
            com = (a.yz * a.x + b.yz * b.x) / mass;
        }
        atomicStore(&node_data[3u * node], bitcast<u32>(mass));
        atomicStore(&node_data[3u * node + 1u], bitcast<u32>(com.x));
        atomicStore(&node_data[3u * node + 2u], bitcast<u32>(com.y));
        packed_nodes[node] = vec4(com, mass, nodes[node].size);
        if node == 0u {
            return;
        }
        node = nodes[node].parent;
    }
}

// --- 6. force ---------------------------------------------------------------------------

// Softened (Plummer) pull of mass m at q on a body at p, without G.
fn pull(p: vec2<f32>, q: vec2<f32>, m: f32) -> vec2<f32> {
    let d = q - p;
    let r2 = dot(d, d) + params.gravity_softening_sq;
    return d * (m * inverseSqrt(r2 * r2 * r2));
}

// Threads go in sorted (Morton) order, so neighboring threads walk similar paths.
@compute @workgroup_size(FORCE_WG)
fn force(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = body_total();
    let s = gid.x;
    if s >= n {
        return;
    }
    let me = sorted_bodies[s];
    let slot = bitcast<u32>(me.w);
    if n < 2u {
        accel[slot] = vec2(0.0);
        return;
    }

    var stack: array<u32, STACK_SIZE>;
    stack[0] = 0u;
    var sp = 1u;
    var a = vec2(0.0);
    while sp > 0u {
        sp -= 1u;
        let c = stack[sp];
        if (c & LEAF) != 0u {
            let b = sorted_bodies[c & ~LEAF];
            a += pull(me.xy, b.xy, b.z);
            continue;
        }
        // (com_x, com_y, mass, size)
        let node = packed_nodes[c];
        let d = node.xy - me.xy;
        // Far enough away (or out of stack): treat the whole node as one mass.
        if node.w * node.w < params.gravity_theta_sq * dot(d, d) || sp + 2u > STACK_SIZE {
            a += pull(me.xy, node.xy, node.z);
        } else {
            stack[sp] = nodes[c].left;
            stack[sp + 1u] = nodes[c].right;
            sp += 2u;
        }
    }
    accel[slot] = params.gravity_strength * a;
}
