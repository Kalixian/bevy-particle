// Particle simulation: `integrate` runs once per slot, then `spawn` once per spawn request.

import package::gpu::shaders::types::{
    Particle, SimParams, Modifier, is_alive, pack_emitter, emitter_slot, emitter_generation
};
import package::gpu::shaders::random::{pcg_hash, rand};
import package::gpu::shaders::modifier_kinds::{
    MOD_ACCELERATION, MOD_POINT_ATTRACTOR, MOD_VORTEX, MOD_DRAG, MOD_BOUNDS,
    MOD_COLOR, MOD_ALPHA, MOD_SIZE, MOD_MASS,
    acceleration_params, point_attractor_params, vortex_params, drag_params, bounds_params,
    color_params, alpha_params, size_params, mass_params
};
import package::gpu::shaders::tracks::{tracks, sample_track, CLOCK_SECONDS};

// Must match `GpuEmitter` in src/emitter.rs. vec2 fields are (start, end) ranges, each
// shaped by its `*_curve` (see `sample_spread`).
struct Emitter {
    position: vec2<f32>,
    spawn_half_extents: vec2<f32>,
    speed: vec2<f32>,
    angle: vec2<f32>,
    radius: vec2<f32>,
    lifetime: vec2<f32>,
    red: vec2<f32>,
    green: vec2<f32>,
    blue: vec2<f32>,
    alpha: vec2<f32>,
    mass_per_radius: f32,
    // Not about this emitter: entry i of the spawn table is kept in array element i, to save a
    // binding. Entries are sorted by first request number, and cover all spaces.
    table_first: u32,
    table_emitter: u32,
    enabled: u32,
    // Particles of other generations belonged to an earlier emitter in this slot.
    generation: u32,
    // Where its forces, constraints and appearance modifiers start in `modifiers`, and the end.
    modifiers: vec4<u32>,
    gravity: u32,
    speed_curve: u32,
    angle_curve: u32,
    radius_curve: u32,
    lifetime_curve: u32,
    red_curve: u32,
    green_curve: u32,
    blue_curve: u32,
    alpha_curve: u32,
}

// A `Spread` shape as a lookup table: t at u = i / (CURVE_SAMPLES - 1), four per vec4.
// Must match `GpuCurve` in src/spread.rs.
const CURVE_SAMPLES: u32 = 32u;
const NO_CURVE: u32 = 0xFFFFFFFFu;
struct Curve {
    samples: array<vec4<f32>, 8>,
}

// Stack of dead slot indices. `count` is signed so that failed pops can briefly
// take it below zero without wrapping (see `spawn`).
struct FreeList {
    count: atomic<i32>,
    indices: array<u32>,
}

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(2) var<storage, read_write> free_list: FreeList;
@group(0) @binding(3) var<storage, read> emitters: array<Emitter>;
@group(0) @binding(4) var<storage, read> modifiers: array<Modifier>;
// Gravity (see src/gpu/gravity.rs). bodies[k] = (x, y, mass, bitcast<f32>(slot)).
@group(0) @binding(5) var<storage, read_write> bodies: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> body_count: atomic<u32>;
@group(0) @binding(7) var<storage, read> accel: array<vec2<f32>>;
@group(0) @binding(8) var<storage, read> curves: array<Curve>;

// Whether `e` is still the emitter that spawned `p`.
fn owns(e: Emitter, p: Particle) -> bool {
    return e.enabled != 0u && e.generation == emitter_generation(p);
}

fn kill(i: u32) {
    particles[i] = Particle();
    let top = atomicAdd(&free_list.count, 1);
    free_list.indices[top] = i;
}

// Packs every live, gravity-participating particle into `bodies`.
// `body_count` is reset to 0 by the CPU before this pass.
@compute @workgroup_size(256)
fn compact(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.capacity {
        return;
    }
    let p = particles[i];
    if !is_alive(p) {
        return;
    }
    let emitter = emitters[emitter_slot(p)];
    if !owns(emitter, p) || emitter.gravity == 0u {
        return;
    }
    let k = atomicAdd(&body_count, 1u);
    // Bodies past the limit are left out of gravity this frame (the gravity passes clamp
    // the count to the same limit).
    if k < params.gravity_max_bodies {
        bodies[k] = vec4(p.position, p.mass, bitcast<f32>(i));
    }
}

@compute @workgroup_size(256)
fn integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.capacity {
        return;
    }

    var p = particles[i];
    if !is_alive(p) {
        return;
    }

    p.age += params.dt;
    let emitter = emitters[emitter_slot(p)];
    if !is_alive(p) || !owns(emitter, p) {
        kill(i);
        return;
    }

    // Gravity was computed this frame for every participant (by `compact` + `gravity`).
    if params.gravity_enabled != 0u && emitter.gravity != 0u {
        p.velocity += accel[i] * params.dt;
    }

    // Start from the spawn appearance, so that scaling doesn't compound across frames.
    p.color = vec4(unpack2x16float(p.base_color.x), unpack2x16float(p.base_color.y));
    p.radius = p.base_radius;
    p.mass = p.base_radius * emitter.mass_per_radius;

    // In each phase, the space's modifiers first, then the particle's own emitter's.
    // Constraints come after all forces, so nothing can push through them.
    apply_forces(&p, params.modifiers);
    apply_forces(&p, emitter.modifiers);
    apply_constraints(&p, params.modifiers);
    apply_constraints(&p, emitter.modifiers);
    apply_appearances(&p, params.modifiers);
    apply_appearances(&p, emitter.modifiers);

    p.position += p.velocity * params.dt;
    particles[i] = p;
}

fn apply_forces(p: ptr<function, Particle>, range: vec4<u32>) {
    for (var k = range.x; k < range.y; k++) {
        apply_force(p, resolve(modifiers[k], *p));
    }
}

fn apply_constraints(p: ptr<function, Particle>, range: vec4<u32>) {
    for (var k = range.y; k < range.z; k++) {
        apply_constraint(p, resolve(modifiers[k], *p));
    }
}

fn apply_appearances(p: ptr<function, Particle>, range: vec4<u32>) {
    for (var k = range.z; k < range.w; k++) {
        apply_appearance(p, modifiers[k]);
    }
}

// Fills in a modifier's animated parameters for one particle.
fn resolve(modifier: Modifier, p: Particle) -> Modifier {
    var m = modifier;
    var header = m.track_offset;
    for (var k = 0u; k < m.track_count; k++) {
        let info = tracks[header].info;
        let value = sample_track(header, p);
        m.a = write_slots(m.a, value, 0, i32(info.z), i32(info.w));
        m.b = write_slots(m.b, value, 4, i32(info.z), i32(info.w));
        header += 1u + info.x;
    }
    return m;
}

// `current` is floats `base .. base + 4` of a modifier's eight; replaces those covered by
// the `width` floats starting at `slot`.
fn write_slots(current: vec4<f32>, value: vec4<f32>, base: i32, slot: i32, width: i32) -> vec4<f32> {
    let i = vec4(0, 1, 2, 3) + vec4(base - slot);
    let covered = (i >= vec4(0)) & (i < vec4(width));
    let c = clamp(i, vec4(0), vec4(3));
    let moved = vec4(value[c.x], value[c.y], value[c.z], value[c.w]);
    return select(current, moved, covered);
}

fn apply_force(p: ptr<function, Particle>, m: Modifier) {
    let dt = params.dt;

    switch m.kind {
        case MOD_ACCELERATION: {
            (*p).velocity += acceleration_params(m).accel * dt;
        }
        case MOD_POINT_ATTRACTOR: {
            let a = point_attractor_params(m);
            let offset = a.center - (*p).position;
            let distance_sq = dot(offset, offset);
            if distance_sq > 0.0 {
                let pull = min(abs(a.strength) / max(distance_sq, 1.0), a.max_accel);
                (*p).velocity += normalize(offset) * pull * sign(a.strength) * dt;
            }
        }
        case MOD_VORTEX: {
            let v = vortex_params(m);
            let offset = (*p).position - v.center;
            let distance = length(offset);
            if distance > 0.0 {
                let tangent = vec2(-offset.y, offset.x) / distance;
                let pull = min(abs(v.strength) / max(distance, 1.0), v.max_accel);
                (*p).velocity += tangent * pull * sign(v.strength) * dt;
            }
        }
        case MOD_DRAG: {
            (*p).velocity *= exp(-drag_params(m).coefficient * dt);
        }
        default: {}
    }
}

fn apply_constraint(p: ptr<function, Particle>, m: Modifier) {
    switch m.kind {
        // Uses the position after this step's motion, to bounce before leaving the box.
        case MOD_BOUNDS: {
            let b = bounds_params(m);
            let lo = b.center - b.half_extents;
            let hi = b.center + b.half_extents;
            let next = (*p).position + (*p).velocity * params.dt;
            if (next.x < lo.x && (*p).velocity.x < 0.0) || (next.x > hi.x && (*p).velocity.x > 0.0) {
                (*p).velocity.x *= -b.restitution;
            }
            if (next.y < lo.y && (*p).velocity.y < 0.0) || (next.y > hi.y && (*p).velocity.y > 0.0) {
                (*p).velocity.y *= -b.restitution;
            }
            (*p).position = clamp((*p).position, lo, hi);
        }
        default: {}
    }
}

fn apply_appearance(p: ptr<function, Particle>, modifier: Modifier) {
    // Over-life tracks mean nothing to immortal particles. (Each kind has one parameter.)
    if modifier.track_count > 0u && (*p).lifetime < 0.0
        && (tracks[modifier.track_offset].info.y & 1u) != CLOCK_SECONDS {
        return;
    }
    let m = resolve(modifier, *p);

    switch m.kind {
        case MOD_COLOR: {
            let c = color_params(m);
            let rgb = c.value * select(vec3(1.0), (*p).color.rgb, c.scale != 0.0);
            (*p).color = vec4(rgb, (*p).color.a);
        }
        case MOD_ALPHA: {
            let a = alpha_params(m);
            (*p).color.a = a.value * select(1.0, (*p).color.a, a.scale != 0.0);
        }
        case MOD_SIZE: {
            let s = size_params(m);
            (*p).radius = s.value * select(1.0, (*p).radius, s.scale != 0.0);
        }
        case MOD_MASS: {
            let s = mass_params(m);
            (*p).mass = s.value * select(1.0, (*p).mass, s.scale != 0.0);
        }
        default: {}
    }
}

@compute @workgroup_size(256)
fn spawn(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.spawn_total {
        return;
    }
    let request = params.spawn_first + id.x;

    // Find the emitter that owns this request: the last table entry starting at or before it.
    var lo = 0u;
    var hi = params.spawn_table_len;
    while hi - lo > 1u {
        let mid = (lo + hi) / 2u;
        if emitters[mid].table_first <= request {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let e = emitters[lo].table_emitter;
    let em = emitters[e];

    // Pop a free slot. Nothing pushes during this pass, so `count` only falls: if the value
    // we took was positive, slot `old - 1` is ours alone. Otherwise the space is full; undo.
    let old = atomicSub(&free_list.count, 1);
    if old <= 0 {
        atomicAdd(&free_list.count, 1);
        return;
    }
    let slot = free_list.indices[u32(old - 1)];

    var rng = pcg_hash(request ^ pcg_hash(params.frame));

    let offset = (vec2(rand(&rng), rand(&rng)) * 2.0 - 1.0) * em.spawn_half_extents;
    let direction = sample_spread(em.angle, em.angle_curve, &rng);
    let speed = sample_spread(em.speed, em.speed_curve, &rng);
    let radius = sample_spread(em.radius, em.radius_curve, &rng);
    var lifetime = -1.0;
    if em.lifetime.x >= 0.0 {
        lifetime = sample_spread(em.lifetime, em.lifetime_curve, &rng);
    }
    let srgb = vec3(
        sample_spread(em.red, em.red_curve, &rng),
        sample_spread(em.green, em.green_curve, &rng),
        sample_spread(em.blue, em.blue_curve, &rng),
    );

    let alpha = sample_spread(em.alpha, em.alpha_curve, &rng);

    var p: Particle;
    p.position = em.position + offset;
    p.velocity = vec2(cos(direction), sin(direction)) * speed;
    p.color = vec4(srgb_to_linear(srgb), alpha);
    p.mass = radius * em.mass_per_radius;
    p.radius = radius;
    p.age = 0.0;
    p.lifetime = lifetime;
    p.emitter = pack_emitter(e, em.generation);
    p.base_radius = radius;
    // 65504 is the largest half float.
    let packed = clamp(p.color, vec4(-65504.0), vec4(65504.0));
    p.base_color = vec2(pack2x16float(packed.xy), pack2x16float(packed.zw));
    // `integrate` won't see this particle until next frame, but it's drawn this frame: apply
    // appearance now so that e.g. a fade-in doesn't flash.
    apply_appearances(&p, params.modifiers);
    apply_appearances(&p, em.modifiers);
    particles[slot] = p;
}

// --- helpers ---

// A random value in `range` (start, end), distributed by `curve` (see src/spread.rs):
// a uniform u is bent through the curve's lookup table, then mapped into the range.
fn sample_spread(range: vec2<f32>, curve: u32, state: ptr<function, u32>) -> f32 {
    return mix(range.x, range.y, shape(curve, rand(state)));
}

fn shape(curve: u32, u: f32) -> f32 {
    if curve == NO_CURVE {
        return u;
    }
    let x = u * f32(CURVE_SAMPLES - 1u);
    let i = min(u32(x), CURVE_SAMPLES - 2u);
    return mix(curve_sample(curve, i), curve_sample(curve, i + 1u), x - f32(i));
}

fn curve_sample(curve: u32, i: u32) -> f32 {
    return curves[curve].samples[i / 4u][i % 4u];
}

// Same curve Bevy uses for `Color::srgba(..).to_linear()`, including values above 1.0.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((max(c, vec3(0.0)) + 0.055) / 1.055, vec3(2.4));
    return select(high, low, c <= vec3(0.04045));
}
