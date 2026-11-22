// Moves every particle one step: space forces -> velocity -> position (semi-implicit Euler).

// Must match `GpuParticle` in src/particle.rs (48-byte stride).
struct Particle {
    position: vec2<f32>,
    velocity: vec2<f32>,
    color: vec4<f32>,
    mass: f32,
    radius: f32,
}

// Must match `SimParams` in src/space.rs.
struct SimParams {
    center: vec2<f32>,
    pull_strength: f32,
    pull_max_accel: f32,
    dt: f32,
    count: u32,
}

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;

@compute @workgroup_size(256)
fn integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count {
        return;
    }

    var p = particles[i];

    // Space-level center pull (placeholder for the phase 3 modifier list).
    let offset = params.center - p.position;
    let distance_sq = dot(offset, offset);
    if distance_sq > 0.0 {
        let accel = clamp(params.pull_strength / max(distance_sq, 1.0), 0.0, params.pull_max_accel);
        p.velocity += normalize(offset) * accel * params.dt;
    }

    p.position += p.velocity * params.dt;
    particles[i] = p;
}
