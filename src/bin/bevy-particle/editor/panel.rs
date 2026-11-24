use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use bevy_particle::{EmitterState, SpaceState};

use super::{
    documents::Documents,
    files::{Files, Notice},
    inspector, theme, Editor,
};
use crate::{
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
                show_files(ui, &scene, &mut documents, &mut files);
                ui.separator();
                if editor.selection.0 != scene {
                    editor.selection = (scene.clone(), default());
                }
                let selection = &mut editor.selection.1;
                inspector::show(ui, &scene, selection, &mut documents, &ages);
            }
            // Keeps the panel as wide as it was dragged.
            ui.take_available_space();
        });
    editor.panel = Some(panel.response.rect);
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

/// Saving, and what there is to say about it.
fn show_files(ui: &mut egui::Ui, scene: &str, documents: &mut Documents, files: &mut Files) {
    if !documents.is_open(scene) {
        ui.weak("Loading…");
    }
    let dirty = documents.dirty_files(Some(scene));
    let conflict = documents.conflict_in(scene);
    ui.horizontal_wrapped(|ui| {
        let save = if cfg!(target_arch = "wasm32") { "Download" } else { "Save" };
        let can_save = !dirty.is_empty() && conflict.is_none();
        let button = ui.add_enabled(can_save, egui::Button::new(save));
        if button.on_hover_text(dirty.join("\n")).clicked() {
            files.save(documents, Some(scene), false);
        }
        let button = ui.add_enabled(!dirty.is_empty(), egui::Button::new("Revert"));
        if button.on_hover_text("Drop the unsaved changes").clicked() {
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
        #[cfg(target_arch = "wasm32")]
        {
            let any = !documents.dirty_files(None).is_empty();
            if ui.add_enabled(any, egui::Button::new("Download all changed")).clicked() {
                files.save(documents, None, false);
            }
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
    show_notice(ui, documents, files);
}

fn show_notice(ui: &mut egui::Ui, documents: &mut Documents, files: &mut Files) {
    let mut dismissed = false;
    let mut confirmed = None;
    match &files.notice {
        None => return,
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
    }
    if dismissed {
        files.notice = None;
    }
    if let Some(scene) = confirmed {
        files.save(documents, scene.as_deref(), true);
    }
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
