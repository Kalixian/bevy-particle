mod camera;
mod demos;
mod emitter;
mod gpu;
mod gravity;
mod modifier;
mod particle;
#[cfg(feature = "profile")]
mod profiling;
mod space;
mod spread;
mod track;

use bevy::prelude::*;

fn main() {
    let mut app = App::new();
    app.add_plugins((
        default_plugins(),
        camera::CameraPlugin,
        modifier::ModifierPlugin,
        space::SpacePlugin,
        emitter::EmitterPlugin,
        gpu::ParticleGpuPlugin,
        demos::DemosPlugin,
    ));

    #[cfg(feature = "profile")]
    app.add_plugins(profiling::ProfilingPlugin);

    app.run();
}

fn default_plugins() -> impl PluginGroup {
    // Profiling measures uncapped frame times, so vsync goes off.
    #[cfg(feature = "profile")]
    return DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            present_mode: bevy::window::PresentMode::AutoNoVsync,
            ..default()
        }),
        ..default()
    });
    #[cfg(not(feature = "profile"))]
    DefaultPlugins
}
