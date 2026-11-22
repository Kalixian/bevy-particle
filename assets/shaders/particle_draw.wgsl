// Draws one quad per particle (6 vertices, one instance each) straight from the particle
// buffer, then cuts it to a disk in the fragment shader.

// Must match `GpuParticle` in src/particle.rs (48-byte stride).
struct Particle {
    position: vec2<f32>,
    velocity: vec2<f32>,
    color: vec4<f32>,
    mass: f32,
    radius: f32,
}

// Only the first field of Bevy's `View` uniform; the rest of the binding is ignored.
struct View {
    clip_from_world: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // -1..1 across the quad.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let p = particles[instance_index];
    let world = p.position + corner * p.radius;

    var out: VertexOutput;
    out.clip_position = view.clip_from_world * vec4(world, 0.0, 1.0);
    out.local = corner;
    out.color = p.color;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if dot(in.local, in.local) > 1.0 {
        discard;
    }
    return in.color;
}
