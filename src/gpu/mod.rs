//! Render-world side of the particle system: compute passes that simulate the particle
//! buffer, and an instanced draw that renders it straight from that buffer.

mod compute;
mod draw;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResourcePlugin, RenderApp},
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
        render_app.insert_resource(ready);
        compute::build(render_app);
        draw::build(render_app);
    }
}

/// Set once the simulation compute passes have run for the first time.
#[derive(Resource, Clone, Default)]
pub struct GpuSimReady(Arc<AtomicBool>);

impl GpuSimReady {
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
