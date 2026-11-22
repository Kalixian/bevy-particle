use bevy::{prelude::*, render::extract_resource::ExtractResource, render::storage::ShaderBuffer};
use bytemuck::{Pod, Zeroable};

/// One particle as stored in the GPU particle buffer.
///
/// Must match `struct Particle` in every shader under `assets/shaders/`.
/// Layout (std430, 48 bytes): position @0, velocity @8, color @16, mass @32, radius @36,
/// then 8 bytes of padding that phase 2 will use for age/lifetime.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GpuParticle {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    /// Linear RGBA (values above 1.0 are HDR and drive bloom).
    pub color: [f32; 4],
    pub mass: f32,
    pub radius: f32,
    pub _padding: [f32; 2],
}

const _: () = assert!(size_of::<GpuParticle>() == 48);

/// The GPU particle buffer and how many particles it holds.
/// Created once at startup from the emitters' bursts (see `space.rs`).
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct ParticleBuffer {
    pub buffer: Handle<ShaderBuffer>,
    pub count: u32,
}
