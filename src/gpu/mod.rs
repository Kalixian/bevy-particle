//! Render-world side of the particle system: compute passes that simulate the particle
//! buffer, and an instanced draw that renders it straight from that buffer.

mod compute;
mod draw;
mod gravity;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResourcePlugin, Extract, ExtractSchedule, RenderApp},
};

use crate::{
    emitter::EmitterUpload, modifier::ModifierUpload, particle::ParticleBuffers, space::SimParams,
};

pub struct ParticleGpuPlugin;

impl Plugin for ParticleGpuPlugin {
    fn build(&self, app: &mut App) {
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

/// Set once the simulation compute passes have run with the current space's buffers.
/// Emitters hold their spawns until then, so nothing is requested before the GPU can act on it.
#[derive(Resource, Clone, Default)]
pub struct GpuSimReady(Arc<AtomicBool>);

impl GpuSimReady {
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Called when the space's buffers are replaced; the next simulated frame sets it again.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
