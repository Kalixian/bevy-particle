// Particle simulation: `integrate` runs once per slot, then `spawn` once per spawn request.

// Must match `GpuParticle` in src/particle.rs (64-byte stride).
// Alive when lifetime < 0 (immortal) or age < lifetime. All-zero = dead.
struct Particle {
    position: vec2<f32>,
    velocity: vec2<f32>,
    color: vec4<f32>,
    mass: f32,
    radius: f32,
    age: f32,
    lifetime: f32,
    emitter: u32,
}

// Must match `SimParams` in src/space.rs.
struct SimParams {
    dt: f32,
    capacity: u32,
    spawn_total: u32,
    emitter_count: u32,
    frame: u32,
    // The space's slice of `modifiers`.
    modifier_offset: u32,
    modifier_count: u32,
    gravity_enabled: u32,
    gravity_strength: f32,
    gravity_softening_sq: f32,
    gravity_method: u32,
    gravity_max_bodies: u32,
    gravity_theta_sq: f32,
}

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
    alpha: f32,
    mass_per_radius: f32,
    spawn_offset: u32,
    spawn_count: u32,
    enabled: u32,
    modifier_offset: u32,
    modifier_count: u32,
    gravity: u32,
    speed_curve: u32,
    angle_curve: u32,
    radius_curve: u32,
    lifetime_curve: u32,
    red_curve: u32,
    green_curve: u32,
    blue_curve: u32,
}

// A `Spread` shape as a lookup table: t at u = i / (CURVE_SAMPLES - 1), four per vec4.
// Must match `GpuCurve` in src/spread.rs.
const CURVE_SAMPLES: u32 = 32u;
const NO_CURVE: u32 = 0xFFFFFFFFu;
struct Curve {
    samples: array<vec4<f32>, 8>,
}

// Must match `GpuModifier` in src/modifier.rs; `a`/`b` are decoded per kind below.
struct Modifier {
    kind: u32,
    a: vec4<f32>,
    b: vec4<f32>,
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

const MOD_ACCELERATION: u32 = 0u;
const MOD_POINT_ATTRACTOR: u32 = 1u;
const MOD_VORTEX: u32 = 2u;
const MOD_DRAG: u32 = 3u;
const MOD_BOUNDS: u32 = 4u;
const MOD_COLOR_OVER_LIFE: u32 = 5u;
const MOD_SIZE_OVER_LIFE: u32 = 6u;
const MOD_MASS_OVER_LIFE: u32 = 7u;

fn is_alive(p: Particle) -> bool {
    return p.lifetime < 0.0 || p.age < p.lifetime;
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
    if !is_alive(p) || emitters[p.emitter].gravity == 0u {
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
    let emitter = emitters[p.emitter];
    if !is_alive(p) || emitter.enabled == 0u {
        kill(i);
        return;
    }

    // Gravity was computed this frame for every participant (by `compact` + `gravity`).
    if params.gravity_enabled != 0u && emitter.gravity != 0u {
        p.velocity += accel[i] * params.dt;
    }

    // Space modifiers first, then the particle's own emitter's.
    apply_modifiers(&p, params.modifier_offset, params.modifier_count);
    apply_modifiers(&p, emitter.modifier_offset, emitter.modifier_count);

    p.position += p.velocity * params.dt;
    particles[i] = p;
}

fn apply_modifiers(p: ptr<function, Particle>, offset: u32, count: u32) {
    for (var k = offset; k < offset + count; k++) {
        apply_modifier(p, modifiers[k]);
    }
}

fn apply_modifier(p: ptr<function, Particle>, m: Modifier) {
    let dt = params.dt;
    // Fraction of life used, or -1 for immortal particles (over-life modifiers skip those).
    var life = -1.0;
    if (*p).lifetime > 0.0 {
        life = saturate((*p).age / (*p).lifetime);
    }

    switch m.kind {
        // a.xy = acceleration
        case MOD_ACCELERATION: {
            (*p).velocity += m.a.xy * dt;
        }
        // a.xy = center (world), a.z = strength, a.w = max accel
        case MOD_POINT_ATTRACTOR: {
            let offset = m.a.xy - (*p).position;
            let distance_sq = dot(offset, offset);
            if distance_sq > 0.0 {
                let accel = min(abs(m.a.z) / max(distance_sq, 1.0), m.a.w) * sign(m.a.z);
                (*p).velocity += normalize(offset) * accel * dt;
            }
        }
        // a.xy = center (world), a.z = strength, a.w = max accel
        case MOD_VORTEX: {
            let offset = (*p).position - m.a.xy;
            let distance = length(offset);
            if distance > 0.0 {
                let tangent = vec2(-offset.y, offset.x) / distance;
                let accel = min(abs(m.a.z) / max(distance, 1.0), m.a.w) * sign(m.a.z);
                (*p).velocity += tangent * accel * dt;
            }
        }
        // a.x = drag coefficient
        case MOD_DRAG: {
            (*p).velocity *= exp(-m.a.x * dt);
        }
        // a.xy = center (world), a.zw = half extents, b.x = restitution.
        // Uses the position after this step's motion so particles never leave the box.
        case MOD_BOUNDS: {
            let lo = m.a.xy - m.a.zw;
            let hi = m.a.xy + m.a.zw;
            let next = (*p).position + (*p).velocity * dt;
            if (next.x < lo.x && (*p).velocity.x < 0.0) || (next.x > hi.x && (*p).velocity.x > 0.0) {
                (*p).velocity.x *= -m.b.x;
            }
            if (next.y < lo.y && (*p).velocity.y < 0.0) || (next.y > hi.y && (*p).velocity.y > 0.0) {
                (*p).velocity.y *= -m.b.x;
            }
            (*p).position = clamp((*p).position, lo, hi);
        }
        // a = start color, b = end color (linear RGBA)
        case MOD_COLOR_OVER_LIFE: {
            if life >= 0.0 {
                (*p).color = mix(m.a, m.b, life);
            }
        }
        // a.x = start radius, a.y = end radius
        case MOD_SIZE_OVER_LIFE: {
            if life >= 0.0 {
                (*p).radius = mix(m.a.x, m.a.y, life);
            }
        }
        // a.x = start mass, a.y = end mass
        case MOD_MASS_OVER_LIFE: {
            if life >= 0.0 {
                (*p).mass = mix(m.a.x, m.a.y, life);
            }
        }
        default: {}
    }
}

@compute @workgroup_size(256)
fn spawn(@builtin(global_invocation_id) id: vec3<u32>) {
    let request = id.x;
    if request >= params.spawn_total {
        return;
    }

    // Find the emitter that owns this request. Linear scan: fine for a handful of emitters.
    var e = 0u;
    for (var k = 0u; k < params.emitter_count; k++) {
        let em = emitters[k];
        if request >= em.spawn_offset && request < em.spawn_offset + em.spawn_count {
            e = k;
            break;
        }
    }
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

    var p: Particle;
    p.position = em.position + offset;
    p.velocity = vec2(cos(direction), sin(direction)) * speed;
    p.color = vec4(srgb_to_linear(srgb), em.alpha);
    p.mass = radius * em.mass_per_radius;
    p.radius = radius;
    p.age = 0.0;
    p.lifetime = lifetime;
    p.emitter = e;
    particles[slot] = p;
}

// --- helpers ---

// PCG hash (Jarzynski & Olano, "Hash Functions for GPU Rendering", 2020).
fn pcg_hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Uniform in [0, 1).
fn rand(state: ptr<function, u32>) -> f32 {
    *state = pcg_hash(*state);
    return f32(*state >> 8u) / 16777216.0;
}

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
