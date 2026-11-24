// Draws one quad per live particle (6 vertices, one instance each) straight from the particle
// buffer, as a disk or with a texture.

import package::gpu::shaders::types::Particle;
import package::gpu::shaders::random::pcg_hash;

// Only the first field of Bevy's `View` uniform; the rest of the binding is ignored.
struct View {
    clip_from_world: mat4x4<f32>,
}

// Must match `GpuRenderParams` in src/render.rs.
struct RenderParams {
    softness: f32,
    stretch: f32,
    z: f32,
    textured: u32,
    // Frame grid of the texture.
    columns: u32,
    rows: u32,
    playback: u32,
    fps: f32,
}

const PLAYBACK_FPS: u32 = 1u;
const PLAYBACK_RANDOM: u32 = 2u;

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var<uniform> settings: RenderParams;
@group(0) @binding(3) var image: texture_2d<f32>;
@group(0) @binding(4) var image_sampler: sampler;
// Slots of the live particles; see live.wesl.
@group(0) @binding(5) var<storage, read> live: array<u32>;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // -1..1 across the quad.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
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
    let slot = live[instance_index];
    let p = particles[slot];

    var out: VertexOutput;
    var offset = corner * p.radius;
    let speed = length(p.velocity);
    if settings.stretch > 0.0 && speed > 0.0 {
        let along = p.velocity / speed;
        let across = vec2(-along.y, along.x);
        offset += vec2(corner.x * 0.5 * speed * settings.stretch, 0.0);
        offset = along * offset.x + across * offset.y;
    }
    out.clip_position = view.clip_from_world * vec4(p.position + offset, settings.z, 1.0);
    out.local = corner;
    out.color = p.color;

    let frame = texture_frame(p, slot);
    let cell = vec2(f32(frame % settings.columns), f32(frame / settings.columns));
    let within = vec2(corner.x, -corner.y) * 0.5 + 0.5;
    out.uv = (cell + within) / vec2(f32(settings.columns), f32(settings.rows));
    return out;
}

fn texture_frame(p: Particle, slot: u32) -> u32 {
    let count = settings.columns * settings.rows;
    if settings.playback == PLAYBACK_FPS {
        return u32(max(p.age * settings.fps, 0.0)) % count;
    }
    if settings.playback == PLAYBACK_RANDOM {
        // Values that stay the same for a particle's whole life.
        return pcg_hash(slot ^ pcg_hash(bitcast<u32>(p.base_radius) ^ p.base_color.x)) % count;
    }
    if p.lifetime > 0.0 {
        return min(u32(p.age / p.lifetime * f32(count)), count - 1u);
    }
    return 0u;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if settings.textured != 0u {
        return in.color * textureSample(image, image_sampler, in.uv);
    }
    let distance = length(in.local);
    if distance > 1.0 {
        discard;
    }
    let fade = clamp((1.0 - distance) / max(settings.softness, 1e-4), 0.0, 1.0);
    return vec4(in.color.rgb, in.color.a * smoothstep(0.0, 1.0, fade));
}
