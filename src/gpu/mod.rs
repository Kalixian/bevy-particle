//! Render-world side of the particle system: compute passes that simulate the particle
//! buffer, and an instanced draw that renders it straight from that buffer.

mod compute;
mod draw;

use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResourcePlugin, RenderApp},
};

use crate::{particle::ParticleBuffer, space::SimParams};

pub struct ParticleGpuPlugin;

impl Plugin for ParticleGpuPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractResourcePlugin::<ParticleBuffer>::default(),
            ExtractResourcePlugin::<SimParams>::default(),
        ));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        compute::build(render_app);
        draw::build(render_app);
    }
}
