// Declarations shared by the particle and gravity shaders.

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
    // The owning emitter; see `emitter_slot` and `emitter_generation`.
    emitter: u32,
    // Spawn values that appearance modifiers start from each frame.
    base_radius: f32,
    base_color: vec2<u32>,
}

// Must match `SimParams` in src/space.rs.
struct SimParams {
    dt: f32,
    capacity: u32,
    // The space's spawn requests are numbered from `spawn_first`.
    spawn_first: u32,
    spawn_total: u32,
    emitter_count: u32,
    frame: u32,
    // Where the space's forces, constraints and appearance modifiers start, and the end.
    modifiers: vec4<u32>,
    gravity_enabled: u32,
    gravity_strength: f32,
    gravity_softening_sq: f32,
    gravity_method: u32,
    gravity_max_bodies: u32,
    gravity_theta_sq: f32,
}

// Must match `GpuModifier` in src/modifier.rs; `a` and `b` hold the parameters,
// unpacked by the functions in the generated `modifier_kinds` module.
struct Modifier {
    kind: u32,
    // This modifier's tracks, back to back in `tracks`.
    track_offset: u32,
    track_count: u32,
    a: vec4<f32>,
    b: vec4<f32>,
}

fn is_alive(p: Particle) -> bool {
    return p.lifetime < 0.0 || p.age < p.lifetime;
}

// Emitter references pack the slot into the low 16 bits and the generation into the high 16.
// Must match `EmitterRegistry` in src/emitter.rs.
fn pack_emitter(slot: u32, generation: u32) -> u32 {
    return slot | (generation << 16u);
}

// Index in the emitter array.
fn emitter_slot(p: Particle) -> u32 {
    return p.emitter & 0xFFFFu;
}

fn emitter_generation(p: Particle) -> u32 {
    return p.emitter >> 16u;
}
