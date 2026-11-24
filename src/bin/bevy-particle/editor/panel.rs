use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use super::{theme, Editor};
use crate::{demos::Demos, input::InputCapture};

const MARGIN: i8 = 8;
const DOTS_SIZE: egui::Vec2 = egui::Vec2::splat(28.0);
const RESTART_WIDTH: f32 = 64.0;

pub(super) fn show(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut demos: ResMut<Demos>,
    capture: Res<InputCapture>,
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

    let window = ctx.viewport_rect();
    let mut root = egui::Ui::new(
        ctx.clone(),
        "editor".into(),
        egui::UiBuilder::new().layer_id(egui::LayerId::background()).max_rect(window),
    );
    // Leave some of the scene visible in narrow windows.
    let max_width = (window.width() - 80.0).clamp(240.0, 640.0);
    let panel = egui::Panel::left("editor panel")
        .default_size(360.0)
        .size_range(240.0..=max_width)
        .frame(egui::Frame::new().fill(theme::PANEL_FILL).inner_margin(MARGIN))
        .show(&mut root, |ui| {
            show_header(ui, &mut editor, &mut demos);
            ui.separator();
            ui.weak("Scene and emitter settings will go here.");
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

fn show_header(ui: &mut egui::Ui, editor: &mut Editor, demos: &mut Demos) {
    ui.horizontal(|ui| {
        if dots_button(ui, "Close the editor (Esc)").clicked() {
            editor.open = false;
        }
        let current = demos.current();
        let mut picked = current;
        let name = demos.names().nth(current).unwrap_or("Loading…").to_string();
        let width = ui.available_width() - RESTART_WIDTH - ui.spacing().item_spacing.x;
        egui::ComboBox::from_id_salt("scene")
            .width(width)
            .height(400.0)
            .truncate()
            .selected_text(name)
            .show_ui(ui, |ui| {
                for (index, name) in demos.names().enumerate() {
                    ui.selectable_value(&mut picked, index, name);
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
