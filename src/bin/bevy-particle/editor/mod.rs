//! The editor: a panel along the left edge, opened with the three dots in the top left corner
//! and closed with them or `Escape`.

mod curves;
mod documents;
mod files;
mod handles;
mod inspector;
mod manage;
mod panel;
mod platform;
mod theme;
mod widgets;

use bevy::{
    camera::{visibility::RenderLayers, CameraOutputMode, Viewport},
    prelude::*,
    render::render_resource::BlendState,
    window::PrimaryWindow,
};
use bevy_egui::{
    egui, EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPostUpdateSet,
    EguiPrimaryContextPass, PrimaryEguiContext,
};

use crate::{
    camera::SceneCamera,
    input::{CaptureSystems, InputCapture},
};

/// In windows narrower than this (logical pixels), the panel covers the scene instead of
/// shrinking it.
const MIN_DOCKED_WINDOW: f32 = 700.0;

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((EguiPlugin::default(), files::FilesPlugin));
        // `spawn_ui_camera` makes the context instead.
        app.world_mut()
            .resource_mut::<EguiGlobalSettings>()
            .auto_create_primary_context = false;
        // Handles are placed with the camera as it'll be drawn.
        app.configure_sets(
            PostUpdate,
            EguiPostUpdateSet::EndPass.after(bevy::camera::CameraUpdateSystems),
        );
        app.init_resource::<Editor>()
            .add_systems(Startup, spawn_ui_camera)
            .add_systems(
                Update,
                (capture_input.in_set(CaptureSystems), (follow_window_size, fit_scene_camera).chain()),
            )
            .add_systems(EguiPrimaryContextPass, panel::show);
    }
}

#[derive(Resource, Default)]
struct Editor {
    open: bool,
    /// Where the panel was last drawn, in logical pixels; `None` while it's closed.
    panel: Option<egui::Rect>,
    themed: bool,
    /// What the inspector shows, of the scene at this path.
    selection: (String, inspector::Selection),
    dialog: Option<manage::Dialog>,
    /// The name of a scene to show once it's on the list.
    show_scene: Option<String>,
}

/// The UI has a camera of its own so that bloom and tonemapping leave it alone, and so that it
/// covers the window whatever the scene camera's viewport is.
fn spawn_ui_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("UI camera"),
        PrimaryEguiContext,
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            // egui's colors are premultiplied.
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            ..default()
        },
        // Draws nothing but the UI. `Msaa` is left as the scene camera has it: 2D cameras of the
        // same window share a depth texture.
        RenderLayers::none(),
    ));
}

fn capture_input(
    mut contexts: EguiContexts,
    editor: Res<Editor>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut capture: ResMut<InputCapture>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // Not `is_pointer_over_egui`: without a root `Ui`, it counts the whole background layer,
    // which the panel is on, as UI.
    let cursor = window.cursor_position().map(|cursor| egui::pos2(cursor.x, cursor.y));
    let over_panel =
        cursor.is_some_and(|cursor| editor.panel.is_some_and(|panel| panel.contains(cursor)));
    let layer = cursor.and_then(|cursor| ctx.layer_id_at(cursor)).map(|layer| layer.order);
    let over_ui = over_panel || layer.is_some_and(|order| order != egui::Order::Background);
    // Handles are in the middle.
    let over_popup = layer
        .is_some_and(|order| !matches!(order, egui::Order::Background | egui::Order::Middle));
    let popup = ctx.any_popup_open();
    *capture = InputCapture {
        pointer: over_ui || popup || ctx.egui_is_using_pointer(),
        scroll: over_panel || over_popup || popup,
        keyboard: popup || ctx.egui_wants_keyboard_input(),
    };
    Ok(())
}

/// Makes the cameras notice a new window size at once. Otherwise they'd wait for the resize event,
/// except that changing a camera's viewport updates that camera alone; until then the viewport may
/// not fit the window, or the cameras disagree on the size of the depth texture they share.
fn follow_window_size(
    window: Single<&Window, With<PrimaryWindow>>,
    mut cameras: Query<(&Camera, &mut Projection)>,
) {
    for (camera, mut projection) in &mut cameras {
        if camera.physical_target_size() != Some(window.physical_size()) {
            // Bevy updates the camera, and fits its viewport, when its projection changes.
            projection.set_changed();
        }
    }
}

/// Limits the scene to the part of the window that the panel leaves free.
fn fit_scene_camera(
    editor: Res<Editor>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<&mut Camera, With<SceneCamera>>,
) {
    let left = editor
        .panel
        .filter(|_| window.width() >= MIN_DOCKED_WINDOW)
        .map(|panel| (panel.right() * window.scale_factor()).round() as u32);
    let rect = left.and_then(|left| free_area(left, window.physical_size()));
    let current = camera.viewport.as_ref().map(|v| (v.physical_position, v.physical_size));
    if rect != current {
        camera.viewport = rect.map(|(physical_position, physical_size)| Viewport {
            physical_position,
            physical_size,
            ..default()
        });
    }
}

/// The position and size of what's right of `left`, in physical pixels. `None` if that's nothing,
/// such as while the window is minimized.
fn free_area(left: u32, window: UVec2) -> Option<(UVec2, UVec2)> {
    (left < window.x && window.y > 0)
        .then(|| (UVec2::new(left, 0), UVec2::new(window.x - left, window.y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_fills_what_the_panel_leaves() {
        let area = free_area(300, UVec2::new(1000, 600));
        assert_eq!(area, Some((UVec2::new(300, 0), UVec2::new(700, 600))));
        assert_eq!(free_area(300, UVec2::new(300, 600)), None);
        assert_eq!(free_area(300, UVec2::ZERO), None);
    }
}
