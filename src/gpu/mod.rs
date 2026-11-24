//! Render-world side of the particle system: compute passes that simulate the particle
//! buffer, and an instanced draw that renders it straight from that buffer.

mod compute;
mod draw;
mod gravity;
#[cfg(test)]
mod shader_tests;

use std::sync::{Arc, Mutex};

use bevy::{
    asset::embedded_asset,
    platform::collections::HashSet,
    prelude::*,
    render::{extract_resource::ExtractResourcePlugin, storage::ShaderBuffer, RenderApp},
    shader::load_shader_library,
};

use crate::{
    emitter::EmitterUpload, modifier::ModifierUpload, modifier_kinds, space::SpaceUploads,
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
        let kinds = Shader::from_wesl(modifier_kinds::wesl_source(), modifier_kinds::WESL_PATH);
        let kinds = app.world_mut().resource_mut::<Assets<Shader>>().add(kinds);
        app.insert_resource(GeneratedShaders { _modifier_kinds: kinds });
        embedded_asset!(app, "shaders/particle_sim.wesl");
        embedded_asset!(app, "shaders/particle_draw.wesl");
        embedded_asset!(app, "shaders/gravity_brute.wesl");
        embedded_asset!(app, "shaders/gravity_bh.wesl");

        app.add_plugins((
            ExtractResourcePlugin::<SpaceUploads>::default(),
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
        gravity::build(render_app);
        compute::build(render_app);
        draw::build(render_app);
    }
}

/// Keeps shader modules without a file alive; others import them by path.
#[derive(Resource)]
struct GeneratedShaders {
    _modifier_kinds: Handle<Shader>,
}

/// The particle buffers simulated in the last rendered frame. Emitters hold their spawns
/// until their space's are among them, so nothing is requested before the GPU can act on it.
///
/// Buffers, rather than space entities, because rendering is pipelined: the render world may
/// still be finishing a frame of a space's previous buffers.
#[derive(Resource, Clone, Default)]
pub struct GpuSimReady(Arc<Mutex<HashSet<AssetId<ShaderBuffer>>>>);

impl GpuSimReady {
    pub fn is_simulating(&self, buffers: AssetId<ShaderBuffer>) -> bool {
        self.0.lock().unwrap().contains(&buffers)
    }

    pub(crate) fn set(&self, simulated: HashSet<AssetId<ShaderBuffer>>) {
        *self.0.lock().unwrap() = simulated;
    }
}
