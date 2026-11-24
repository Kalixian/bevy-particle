use bevy::{prelude::*, render::storage::ShaderBuffer};
use bytemuck::{Pod, Zeroable};

/// One particle slot in the GPU particle buffer.
///
/// Must match `struct Particle` in `types.wesl`.
/// Layout (std430, 64 bytes): position @0, velocity @8, color @16, mass @32, radius @36,
/// age @40, lifetime @44, emitter @48, base_radius @52, base_color @56.
///
/// A slot is alive when `lifetime < 0` (immortal) or `age < lifetime`.
/// An all-zero slot is dead, so a zero-initialized buffer starts empty.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GpuParticle {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    /// Linear RGBA (values above 1.0 are HDR and drive bloom).
    pub color: [f32; 4],
    pub mass: f32,
    pub radius: f32,
    /// Seconds since spawn.
    pub age: f32,
    /// Seconds the particle lives; negative means it never dies.
    pub lifetime: f32,
    /// The owning emitter: its slot in the GPU emitter array in the low 16 bits and the
    /// slot's generation in the high 16 (see `EmitterRegistry`).
    pub emitter: u32,
    /// Spawn values. Every frame color, radius and mass (= radius × the emitter's
    /// `mass_per_radius`) start from these before appearance modifiers apply.
    pub base_radius: f32,
    /// Linear RGBA packed as four half floats.
    pub base_color: [u32; 2],
}

const _: () = assert!(size_of::<GpuParticle>() == 64);

/// The GPU buffers backing a particle space, on the space entity.
///
/// - `particles`: `capacity` slots of [`GpuParticle`], zero-initialized (all dead).
/// - `free_list`: `[count: i32, indices: [u32; capacity]]`, a stack of dead slot indices.
///   Starts full. The integrate pass pushes slots as particles die; the spawn pass pops them.
#[derive(Component, Clone, Debug)]
pub struct ParticleBuffers {
    pub particles: Handle<ShaderBuffer>,
    pub free_list: Handle<ShaderBuffer>,
    pub capacity: u32,
}

impl ParticleBuffers {
    /// Identifies this set of buffers. Unlike the space entity, it changes if the space's
    /// buffers are ever recreated.
    pub fn id(&self) -> AssetId<ShaderBuffer> {
        self.particles.id()
    }
}
