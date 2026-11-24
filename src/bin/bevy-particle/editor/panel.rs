use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use bevy_particle::{EmitterState, SpaceState};

use super::{
    documents::Documents,
    files::{Files, Notice},
    handles, inspector, manage, theme, Editor,
};
use crate::{
    camera::SceneCamera,
    demos::{effect_path, Demos},
    input::InputCapture,
};

const MARGIN: i8 = 8;
const DOTS_SIZE: egui::Vec2 = egui::Vec2::splat(28.0);
/// Fits the widest rows of the inspector.
const MIN_WIDTH: f32 = 310.0;
const RESTART_WIDTH: f32 = 64.0;

pub(super) fn show(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut demos: ResMut<Demos>,
    mut documents: ResMut<Documents>,
    mut files: ResMut<Files>,
    capture: Res<InputCapture>,
    spaces: Query<(&SpaceState, Option<&Children>)>,
    emitters: Query<&EmitterState>,
    camera: Single<(&Camera, &Transform), With<SceneCamera>>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !editor.themed {
        theme::apply(ctx);
        editor.themed = true;
    }

    // While something else has the keyboard, Escape is for that (closing a menu, say).
    let escape = !capture.keyboard && ctx.input(|input| input.key_pressed(egui::Key::Escape));
    if escape {
        editor.open = false;
    }
    if !editor.open {
        editor.panel = None;
        show_closed(ctx, &mut editor);
        return Ok(());
    }

    let ages = match spaces.single() {
        Ok((space, children)) => inspector::Ages {
            scene: Some(space.age()),
            emitters: children
                .into_iter()
                .flatten()
                .filter_map(|child| Some(emitters.get(*child).ok()?.age()))
                .collect(),
        },
        Err(_) => default(),
    };
    let created = editor.show_scene.as_deref();
    let created = demos.names().position(|name| Some(name) == created);
    if let Some(index) = created {
        demos.show(index);
        editor.show_scene = None;
    }

    let window = ctx.viewport_rect();
    let mut root = egui::Ui::new(
        ctx.clone(),
        "editor".into(),
        egui::UiBuilder::new().layer_id(egui::LayerId::background()).max_rect(window),
    );
    // Leave some of the scene visible in narrow windows.
    let max_width = (window.width() - 80.0).clamp(MIN_WIDTH, 640.0);
    let panel = egui::Panel::left("editor panel")
        .default_size(360.0)
        .size_range(MIN_WIDTH..=max_width)
        .frame(egui::Frame::new().fill(theme::PANEL_FILL).inner_margin(MARGIN))
        .show(&mut root, |ui| {
            show_header(ui, &mut editor, &mut demos, &documents);
            let scene = demos.names().nth(demos.current()).map(effect_path);
            if let Some(scene) = scene {
                undo_shortcuts(ctx, &scene, &mut documents);
                let Editor { dialog, show_scene, .. } = &mut *editor;
                if show_files(ui, &scene, &mut documents, &mut files, dialog) {
                    exit.write(AppExit::Success);
                }
                let created = manage::show_dialog(ui, &scene, &mut documents, dialog);
                *show_scene = created.or(show_scene.take());
                ui.separator();
                if editor.selection.0 != scene {
                    editor.selection = (scene.clone(), default());
                    editor.dialog = None;
                }
                let Editor { selection: (_, selection), dialog, .. } = &mut *editor;
                inspector::show(ui, &scene, selection, &mut documents, &ages, dialog);
            }
            // Keeps the panel as wide as it was dragged.
            ui.take_available_space();
        });
    let panel = panel.response.rect;
    editor.panel = Some(panel);
    if editor.open {
        let area = window.with_min_x(panel.right());
        let (scene, selection) = &editor.selection;
        // Not its `GlobalTransform`, which may be from before it was panned this frame.
        let (camera, transform) = *camera;
        let camera = (camera, &GlobalTransform::from(*transform));
        handles::show(ctx, scene, *selection, &mut documents, &ages, camera, area);
    }
    Ok(())
}

/// The three dots by themselves, where the header puts them when the panel is open.
fn show_closed(ctx: &egui::Context, editor: &mut Editor) {
    egui::Area::new("editor dots".into())
        .fixed_pos(egui::Pos2::ZERO + egui::Vec2::splat(MARGIN.into()))
        .show(ctx, |ui| {
            if dots_button(ui, "Open the editor").clicked() {
                editor.open = true;
            }
        });
}

fn show_header(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    demos: &mut Demos,
    documents: &Documents,
) {
    ui.horizontal(|ui| {
        if dots_button(ui, "Close the editor (Esc)").clicked() {
            editor.open = false;
        }
        let current = demos.current();
        let mut picked = current;
        // Scenes with unsaved changes are marked.
        let label = |name: &str| {
            let unsaved = !documents.dirty_files(Some(&effect_path(name))).is_empty();
            format!("{name}{}", if unsaved { " *" } else { "" })
        };
        let name = demos.names().nth(current).map_or("Loading…".into(), label);
        let width = ui.available_width() - RESTART_WIDTH - ui.spacing().item_spacing.x;
        egui::ComboBox::from_id_salt("scene")
            .width(width)
            .height(400.0)
            .truncate()
            .selected_text(name)
            .show_ui(ui, |ui| {
                for (index, name) in demos.names().enumerate() {
                    ui.selectable_value(&mut picked, index, label(name));
                }
            });
        if picked != current {
            demos.show(picked);
        }
        let restart = egui::Button::new("Restart").min_size(egui::vec2(RESTART_WIDTH, 0.0));
        if ui.add(restart).on_hover_text("Restart the scene (R)").clicked() {
            demos.restart();
        }
    });
}

fn undo_shortcuts(ctx: &egui::Context, scene: &str, documents: &mut Documents) {
    // Text fields have their own.
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let pressed = |modifiers, key| {
        ctx.input_mut(|input| input.consume_shortcut(&egui::KeyboardShortcut::new(modifiers, key)))
    };
    let command = egui::Modifiers::COMMAND;
    // First, since shortcuts ignore extra modifiers.
    if pressed(command | egui::Modifiers::SHIFT, egui::Key::Z) || pressed(command, egui::Key::Y) {
        documents.redo(scene);
    } else if pressed(command, egui::Key::Z) {
        documents.undo(scene);
    }
}

/// Saving, and what there is to say about it. Returns whether to quit.
fn show_files(
    ui: &mut egui::Ui,
    scene: &str,
    documents: &mut Documents,
    files: &mut Files,
    dialog: &mut Option<manage::Dialog>,
) -> bool {
    if !documents.is_open(scene) {
        ui.weak("Loading…");
    }
    let dirty = documents.dirty_files(Some(scene));
    let conflict = documents.conflict_with(scene);
    ui.horizontal_wrapped(|ui| {
        let save = if cfg!(target_arch = "wasm32") { "Download" } else { "Save" };
        let to_save = documents.files_to_save(scene);
        let can_save = !to_save.is_empty() && conflict.is_none();
        let button = ui.add_enabled(can_save, egui::Button::new(save));
        if button.on_hover_text(to_save.join("\n")).clicked() {
            files.save(documents, Some(scene), false);
        }
        let can_revert = dirty.iter().any(|path| documents.can_revert(path));
        let button = ui.add_enabled(can_revert, egui::Button::new("Revert"));
        if button.on_hover_text("Drop the unsaved changes to this scene").clicked() {
            for path in &dirty {
                documents.revert(path);
            }
            documents.end_group();
            files.notice = None;
        }
        let button = ui.add_enabled(documents.can_undo(scene), egui::Button::new("Undo"));
        if button.on_hover_text("Ctrl+Z").clicked() {
            documents.undo(scene);
        }
        let button = ui.add_enabled(documents.can_redo(scene), egui::Button::new("Redo"));
        if button.on_hover_text("Ctrl+Shift+Z or Ctrl+Y").clicked() {
            documents.redo(scene);
        }
        manage::scene_menu(ui, scene, documents, dialog);
        let more = documents.dirty_files(None).iter().any(|path| !to_save.contains(path));
        if more && ui.button(format!("{save} all")).clicked() {
            files.save(documents, None, false);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let button = ui.button("Load files…");
            let hint = "Replaces scenes and emitters by downloaded ones with the same name";
            if button.on_hover_text(hint).clicked() {
                files.notice = super::platform::pick_files()
                    .err()
                    .map(|error| Notice::Error(error.to_string()));
            }
        }
    });

    for path in &dirty {
        let mut others = documents.scenes_using(path);
        others.retain(|other| other != scene);
        if !others.is_empty() {
            let others = others.join(", ");
            ui.colored_label(theme::WARNING, format!("{path} is also used by {others}."));
        }
    }
    if let Some(path) = conflict {
        ui.colored_label(theme::WARNING, format!("{path} was changed outside the editor."));
        ui.horizontal(|ui| {
            let mine = ui.button("Keep mine").clicked();
            if (mine || ui.button("Use the file's").clicked())
                && let Err(error) = documents.resolve(&path, mine)
            {
                files.notice = Some(Notice::Error(error.to_string()));
            }
        });
    }
    show_notice(ui, documents, files)
}

/// Returns whether to quit.
fn show_notice(ui: &mut egui::Ui, documents: &mut Documents, files: &mut Files) -> bool {
    let mut quit = false;
    let mut save_all = false;
    let mut dismissed = false;
    let mut confirmed = None;
    match &files.notice {
        None => return false,
        Some(Notice::Info(text)) => {
            ui.weak(text);
            dismissed = ui.small_button("OK").clicked();
        }
        Some(Notice::Error(text)) => {
            ui.colored_label(theme::ERROR, text);
            dismissed = ui.small_button("OK").clicked();
        }
        Some(Notice::Comments { scene, files }) => {
            let text = format!("Saving removes the comments in {}.", files.join(", "));
            ui.colored_label(theme::WARNING, text);
            ui.horizontal(|ui| {
                if ui.button("Save anyway").clicked() {
                    confirmed = Some(scene.clone());
                }
                dismissed = ui.button("Cancel").clicked();
            });
        }
        Some(Notice::Close) => {
            let dirty = documents.dirty_files(None);
            let text = format!("There are unsaved changes in {}.", dirty.join(", "));
            ui.colored_label(theme::WARNING, text);
            let commented: Vec<_> =
                dirty.into_iter().filter(|path| documents.has_comments(path)).collect();
            if !commented.is_empty() {
                let text = format!("Saving removes the comments in {}.", commented.join(", "));
                ui.colored_label(theme::WARNING, text);
            }
            ui.horizontal_wrapped(|ui| {
                save_all = ui.button("Save all and quit").clicked();
                quit = ui.button("Quit without saving").clicked();
                dismissed = ui.button("Cancel").clicked();
            });
        }
    }
    if dismissed {
        files.notice = None;
    }
    if let Some(scene) = confirmed {
        files.save(documents, scene.as_deref(), true);
    }
    if save_all {
        quit = files.save(documents, None, true);
    }
    quit
}

/// The dots are drawn, rather than being text, so that they don't depend on the font.
fn dots_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(DOTS_SIZE, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let visuals = ui.style().interact(&response);
    let active = response.hovered() || response.has_focus();
    let fill = if active { visuals.bg_fill } else { theme::BUTTON_FILL };
    ui.painter().rect_filled(rect, 6.0, fill);
    for offset in [-6.0, 0.0, 6.0] {
        let center = rect.center() + egui::vec2(0.0, offset);
        ui.painter().circle_filled(center, 1.75, visuals.fg_stroke.color);
    }
    response.on_hover_text(label)
}
