//! The demo app: shows the effects in `assets/`.

mod camera;
mod cursor;
mod demos;
#[cfg(feature = "editor")]
mod editor;
mod input;
#[cfg(feature = "profile")]
mod profiling;

use bevy::prelude::*;

fn main() {
    let mut app = App::new();
    app.init_resource::<input::InputCapture>().add_plugins((
        default_plugins(),
        bevy_particle::ParticlePlugin,
        camera::CameraPlugin,
        cursor::CursorPlugin,
        demos::DemosPlugin,
    ));

    #[cfg(feature = "editor")]
    app.add_plugins(editor::EditorPlugin);

    #[cfg(feature = "profile")]
    app.add_plugins(profiling::ProfilingPlugin);

    app.run();
}

fn default_plugins() -> impl PluginGroup {
    DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(Window {
                // Profiling measures uncapped frame times, so vsync goes off.
                #[cfg(feature = "profile")]
                present_mode: bevy::window::PresentMode::AutoNoVsync,
                // Web only: fill the page's canvas (see web/index.html), and leave browser
                // shortcuts such as F5 and F12 working.
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                prevent_default_event_handling: false,
                ..default()
            }),
            ..default()
        })
        .set(AssetPlugin {
            // There are no .meta files; looking for them costs a failed request per asset on
            // the web, and confuses hosts that answer missing files with a page.
            meta_check: bevy::asset::AssetMetaCheck::Never,
            ..default()
        })
}
