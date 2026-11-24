use bevy_egui::egui::{self, Color32};

pub const PANEL_FILL: Color32 = Color32::from_rgb(24, 25, 28);
pub const BUTTON_FILL: Color32 = Color32::from_rgba_premultiplied(20, 21, 24, 225);

pub const WARNING: Color32 = Color32::from_rgb(230, 180, 80);
pub const KEYED: Color32 = Color32::from_rgb(110, 190, 255);
pub const ERROR: Color32 = Color32::from_rgb(240, 110, 100);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.visuals.panel_fill = PANEL_FILL;
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.interact_size.y = 24.0;
    });
}
