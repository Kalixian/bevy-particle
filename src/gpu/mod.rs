//! Render-world side of the particle system: compute passes that simulate the particle
//! buffer, and an instanced draw that renders it straight from that buffer.

mod compute;
mod draw;
mod gravity;
#[cfg(test)]
mod shader_tests;

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use bevy::{
    asset::embedded_asset,
    prelude::*,
    render::{extract_resource::ExtractResourcePlugin, Extract, ExtractSchedule, RenderApp},
    shader::load_shader_library,
};

use crate::{
    emitter::EmitterUpload, modifier::ModifierUpload, particle::ParticleBuffers, space::SimParams,
};

pub struct ParticleGpuPlugin;

impl Plugin for ParticleGpuPlugin {
    fn build(&self, app: &mut App) {
        // Embedded rather than loaded from `assets/`: WESL imports don't tell modules from
        // items, so Bevy would probe for asset files that don't exist (failed requests on the
        // web). List every shader in `shader_tests.rs` too.
        load_shader_library!(app, "shaders/types.wesl");
        load_shader_library!(app, "shaders/random.wesl");
        load_shader_library!(app, "shaders/tracks.wesl");
        embedded_asset!(app, "shaders/particle_sim.wesl");
        embedded_asset!(app, "shaders/particle_draw.wesl");
        embedded_asset!(app, "shaders/gravity_brute.wesl");
        embedded_asset!(app, "shaders/gravity_bh.wesl");

        app.add_plugins((
            ExtractResourcePlugin::<ParticleBuffers>::default(),
            ExtractResourcePlugin::<SimParams>::default(),
            ExtractResourcePlugin::<EmitterUpload>::default(),
            ExtractResourcePlugin::<ModifierUpload>::default(),
        ));

        // The same flag lives in both worlds: the render world sets it, the main world reads it.
        let ready = GpuSimReady::default();
        app.insert_resource(ready.clone());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(ready)
            .add_systems(ExtractSchedule, extract_buffer_removal);
        gravity::build(render_app);
        compute::build(render_app);
        draw::build(render_app);
    }
}

/// `ExtractResourcePlugin` copies resources over but never removes them, so without this a
/// removed space's buffers would keep being simulated and drawn.
fn extract_buffer_removal(
    mut commands: Commands,
    main_buffers: Extract<Option<Res<ParticleBuffers>>>,
    render_buffers: Option<Res<ParticleBuffers>>,
) {
    if main_buffers.is_none() && render_buffers.is_some() {
        commands.remove_resource::<ParticleBuffers>();
    }
}

/// The space whose buffers the simulation compute passes last ran with. Emitters hold their
/// spawns until it's theirs, so nothing is requested before the GPU can act on it.
///
/// It names the space, rather than being a flag reset for each new one, because rendering is
/// pipelined: the render world may still be finishing a frame of the previous space when the
/// main world creates the next.
#[derive(Resource, Clone)]
pub struct GpuSimReady(Arc<AtomicU64>);

impl Default for GpuSimReady {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(Entity::PLACEHOLDER.to_bits())))
    }
}

impl GpuSimReady {
    /// Whether `space` has been simulated with its current buffers.
    pub fn is_simulating(&self, space: Entity) -> bool {
        self.0.load(Ordering::Relaxed) == space.to_bits()
    }

    fn set(&self, space: Entity) {
        self.0.store(space.to_bits(), Ordering::Relaxed);
    }
}
