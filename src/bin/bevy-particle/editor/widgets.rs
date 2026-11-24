//! Controls for each type of value. They only write to a value when the user changes it, so that
//! showing a value never alters it.

use std::ops::RangeInclusive;

use bevy::{math::Vec2, prelude::Color};
use bevy_egui::egui::{
    self,
    containers::menu::{MenuButton, MenuConfig},
    PopupCloseBehavior, Response, Sense, Ui,
};
use bevy_particle::{Rgb, Shape, Spread};

const LABEL_WIDTH: f32 = 92.0;
const NUMBER_WIDTH: f32 = 64.0;
const ROW_HEIGHT: f32 = 22.0;
const KEY_WIDTH: f32 = 18.0;

/// How a number is shown and what it can be.
#[derive(Clone)]
pub struct Number {
    /// What the slider covers; typed values can be outside of it.
    soft: RangeInclusive<f32>,
    /// What any value is limited to.
    hard: RangeInclusive<f32>,
    /// Has the slider give small values more room.
    log: bool,
    /// Shown value = value × this.
    scale: f32,
    suffix: &'static str,
    decimals: usize,
}

impl Number {
    /// In shown units.
    pub const fn new(min: f32, max: f32) -> Self {
        Self {
            soft: min..=max,
            hard: f32::NEG_INFINITY..=f32::INFINITY,
            log: false,
            scale: 1.0,
            suffix: "",
            decimals: 4,
        }
    }

    pub const fn log(mut self) -> Self {
        self.log = true;
        self
    }

    pub const fn at_least(mut self, min: f32) -> Self {
        self.hard = min..=*self.hard.end();
        self
    }

    pub const fn at_most(mut self, max: f32) -> Self {
        self.hard = *self.hard.start()..=max;
        self
    }

    pub const fn suffix(mut self, suffix: &'static str) -> Self {
        self.suffix = suffix;
        self
    }

    /// Radians, shown as degrees.
    pub const fn angle(min: f32, max: f32) -> Self {
        let mut number = Self::new(min, max).suffix("°");
        number.scale = 180.0 / std::f32::consts::PI;
        number
    }

    const fn whole(mut self) -> Self {
        self.decimals = 0;
        self
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn suffix_text(&self) -> &'static str {
        self.suffix
    }

    /// How much the slider covers, in shown units.
    pub fn span(&self) -> f32 {
        self.soft.end() - self.soft.start()
    }

    /// Where a shown value is on the slider, from 0 to 1.
    fn position(&self, shown: f32) -> f32 {
        let (min, max) = (self.bend(*self.soft.start()), self.bend(*self.soft.end()));
        ((self.bend(shown) - min) / (max - min)).clamp(0.0, 1.0)
    }

    /// The inverse of [`Self::position`].
    fn at(&self, position: f32) -> f32 {
        let (min, max) = (self.bend(*self.soft.start()), self.bend(*self.soft.end()));
        self.unbend(min + (max - min) * position.clamp(0.0, 1.0))
    }

    /// Values well below this are about linear on a logarithmic slider.
    fn knee(&self) -> f32 {
        self.soft.start().abs().max(self.soft.end().abs()) * 1e-3
    }

    fn bend(&self, shown: f32) -> f32 {
        if self.log { (shown / self.knee()).asinh() } else { shown }
    }

    fn unbend(&self, bent: f32) -> f32 {
        if self.log { bent.sinh() * self.knee() } else { bent }
    }

    /// The closest value to this that's allowed.
    pub fn limit(&self, value: f32) -> f32 {
        value.clamp(*self.hard.start(), *self.hard.end())
    }

    /// Sets a value from a shown one, unless that isn't a number.
    pub fn store(&self, value: &mut f32, shown: f32) {
        if shown.is_finite() {
            *value = (shown / self.scale).clamp(*self.hard.start(), *self.hard.end());
        }
    }

    /// Rounds to what a slider `width` points wide can show, so that dragging makes for tidy
    /// numbers.
    fn tidy(&self, shown: f32, position: f32, width: f32) -> f32 {
        let pixel = 1.0 / width.max(1.0);
        let step = (self.at(position + pixel) - self.at(position - pixel)).abs() / 2.0;
        round_to_step(shown, step)
    }
}

/// Rounds to a decimal place that's at least as fine as `step`.
pub fn round_to_step(value: f32, step: f32) -> f32 {
    if step.is_nan() || step <= 0.0 {
        return value;
    }
    let unit = 10f64.powf(f64::from(step).log10().floor());
    ((f64::from(value) / unit).round() * unit) as f32
}

/// A labeled row.
pub fn row<R>(ui: &mut Ui, label: &str, hint: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let size = egui::vec2(LABEL_WIDTH, ROW_HEIGHT);
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(size, layout, |ui| {
            ui.set_min_width(LABEL_WIDTH);
            let label = ui.add(egui::Label::new(label).truncate());
            if !hint.is_empty() {
                label.on_hover_text(hint);
            }
        });
        add(ui)
    })
    .inner
}

/// A labeled row with a keyframe button, which `button` shows.
pub fn key_row<R>(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut Ui),
    button: impl FnOnce(&mut Ui) -> R,
) -> R {
    row(ui, label, hint, |ui| {
        let width = ui.available_width() - KEY_WIDTH - ui.spacing().item_spacing.x;
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(width, ROW_HEIGHT), layout, |ui| {
            ui.set_min_width(width);
            add(ui);
        });
        button(ui)
    })
}

/// A number to drag or type.
pub fn drag(ui: &mut Ui, value: &mut f32, number: &Number) -> Response {
    let mut shown = *value * number.scale;
    let span = number.span();
    let speed = if number.log { shown.abs().max(number.knee()) * 0.01 } else { span * 0.002 };
    let widget = egui::DragValue::new(&mut shown)
        .speed(speed)
        .max_decimals(number.decimals)
        .suffix(number.suffix);
    let response = ui.add_sized([NUMBER_WIDTH, ROW_HEIGHT], widget);
    if response.changed() {
        number.store(value, shown);
    }
    response
}

/// A slider and a number, filling the rest of the row.
pub fn slider(ui: &mut Ui, value: &mut f32, number: &Number) -> [Response; 2] {
    let width = ui.available_width() - NUMBER_WIDTH - ui.spacing().item_spacing.x;
    let response = rail(ui, width, number, &mut [value]);
    [response, drag(ui, value, number)]
}

/// A slider with a handle for each value.
fn rail(ui: &mut Ui, width: f32, number: &Number, values: &mut [&mut f32]) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width.max(20.0), ROW_HEIGHT), Sense::click_and_drag());
    let rect = rect.shrink2(egui::vec2(7.0, 0.0));
    let x = |value: f32| rect.left() + rect.width() * number.position(value * number.scale);

    // A drag moves the handle that it started closest to.
    let grabbed = response.id.with("grabbed");
    let pointer = response.interact_pointer_pos();
    if let Some(pointer) = pointer {
        if response.drag_started() || response.clicked() {
            let distance = |value: &f32| (x(*value) - pointer.x).abs();
            let mut nearest = 0;
            for i in 1..values.len() {
                let (this, best) = (distance(values[i]), distance(values[nearest]));
                // Of handles in the same place, take the one that can move this way.
                if this < best || (this == best && pointer.x > x(*values[i])) {
                    nearest = i;
                }
            }
            ui.data_mut(|data| data.insert_temp(grabbed, nearest));
        }
        if response.dragged() || response.clicked() {
            let index = ui.data(|data| data.get_temp(grabbed)).unwrap_or(0usize);
            if let Some(value) = values.get_mut(index) {
                let position = (pointer.x - rect.left()) / rect.width();
                let shown = number.tidy(number.at(position), position, rect.width());
                // Holding still isn't a change, even if the value is off the slider's end.
                if response.clicked() || response.drag_delta().x != 0.0 {
                    number.store(value, shown);
                }
            }
        }
    }

    let visuals = ui.style().interact(&response);
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let painter = ui.painter();
    painter.rect_filled(track, 2.0, ui.visuals().widgets.inactive.bg_fill);
    let xs: Vec<_> = values.iter().map(|value| x(**value)).collect();
    if let [a, b] = xs[..] {
        let filled = egui::Rect::from_x_y_ranges(a.min(b)..=a.max(b), track.y_range());
        painter.rect_filled(filled, 2.0, ui.visuals().selection.bg_fill);
    }
    for x in xs {
        painter.circle(egui::pos2(x, rect.center().y), 6.0, visuals.bg_fill, visuals.fg_stroke);
    }
    response
}

pub fn number(ui: &mut Ui, label: &str, hint: &str, value: &mut f32, number: &Number) {
    row(ui, label, hint, |ui| slider(ui, value, number));
}

/// A whole number that only takes effect when it's let go of.
pub fn integer_on_release(ui: &mut Ui, label: &str, hint: &str, value: &mut u32, number: &Number) {
    row(ui, label, hint, |ui| {
        let id = ui.id().with(label);
        let draft = ui.data(|data| data.get_temp::<f32>(id));
        let mut shown = draft.unwrap_or(*value as f32);
        let responses = slider(ui, &mut shown, &number.clone().whole());
        let active = responses.iter().any(|r| r.dragged() || r.has_focus());
        let changed = responses.iter().any(|r| r.changed()) || shown != *value as f32;
        if active {
            ui.data_mut(|data| data.insert_temp(id, shown));
        } else {
            ui.data_mut(|data| data.remove::<f32>(id));
            if changed {
                *value = shown.round().max(0.0) as u32;
            }
        }
    });
}

pub fn integer(ui: &mut Ui, label: &str, hint: &str, value: &mut u32, number: &Number) {
    row(ui, label, hint, |ui| {
        let mut shown = *value as f32;
        slider(ui, &mut shown, &number.clone().whole());
        if shown != *value as f32 {
            *value = shown.round().max(0.0) as u32;
        }
    });
}

pub fn checkbox(ui: &mut Ui, label: &str, hint: &str, value: &mut bool) {
    row(ui, label, hint, |ui| ui.checkbox(value, ""));
}

pub fn vec2(ui: &mut Ui, value: &mut Vec2, number: &Number) {
    ui.weak("x");
    drag(ui, &mut value.x, number);
    ui.weak("y");
    drag(ui, &mut value.y, number);
}

/// A dropdown of values.
pub fn choice<T: PartialEq + Copy>(ui: &mut Ui, id: &str, value: &mut T, options: &[(T, &str)]) {
    let current = options.iter().find(|(option, _)| option == value);
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.map_or("", |(_, name)| name))
        .show_ui(ui, |ui| {
            for (option, name) in options {
                ui.selectable_value(value, *option, *name);
            }
        });
}

/// A dropdown of an enum's variants, each with a value to start from. Switching back to a variant
/// brings back what it was.
pub fn variant<T: Clone + Send + Sync + 'static>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    options: &[(&str, T)],
) {
    let same = |a: &T, b: &T| std::mem::discriminant(a) == std::mem::discriminant(b);
    let Some(current) = options.iter().position(|(_, option)| same(option, value)) else {
        return;
    };
    let mut picked = current;
    let salt = ui.id().with(id);
    egui::ComboBox::from_id_salt(id).selected_text(options[current].0).show_ui(ui, |ui| {
        for (index, (name, _)) in options.iter().enumerate() {
            ui.selectable_value(&mut picked, index, *name);
        }
    });
    if picked != current {
        let old = std::mem::replace(
            value,
            ui.data(|data| data.get_temp(salt.with(picked)))
                .unwrap_or_else(|| options[picked].1.clone()),
        );
        ui.data_mut(|data| data.insert_temp(salt.with(current), old));
    }
}

/// A checkbox for whether there's a value. Switching it back on brings back what it was.
pub fn optional<T: Clone + Send + Sync + 'static>(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Option<T>,
    default: T,
) {
    row(ui, label, hint, |ui| {
        let id = ui.id().with(label);
        let mut on = value.is_some();
        if ui.checkbox(&mut on, "").changed() {
            if on {
                *value = Some(ui.data(|data| data.get_temp(id)).unwrap_or(default));
            } else if let Some(old) = value.take() {
                ui.data_mut(|data| data.insert_temp(id, old));
            }
        }
    });
}

/// The ends of a random range.
pub fn spread_ends(ui: &mut Ui, value: &mut Spread, number: &Number) {
    drag(ui, &mut value.range.start, number);
    ui.weak("to");
    drag(ui, &mut value.range.end, number);
}

/// A two-handled slider for a random range, if it has constant ends, and how its values are
/// distributed.
pub fn spread_shape(ui: &mut Ui, value: &mut Spread, number: &Number, constant: bool) {
    row(ui, "", "", |ui| {
        if constant {
            let range = &mut value.range;
            let width = (ui.available_width() - 92.0).max(40.0);
            rail(ui, width, number, &mut [&mut range.start, &mut range.end]);
        }
        shape(ui, &mut value.shape);
    });
}

/// The dropdown of a distribution, with its strength in a popup of its own.
fn shape(ui: &mut Ui, value: &mut Shape) {
    if let Shape::Custom(_) = value {
        ui.weak("custom").on_hover_text("Set by code; it can't be edited here");
        return;
    }
    let name = |shape: &Shape| match shape {
        Shape::Uniform => "Uniform",
        Shape::Low(_) => "Low",
        Shape::High(_) => "High",
        Shape::Middle(_) => "Middle",
        Shape::Falloff(_) => "Falloff",
        Shape::Custom(_) => "Custom",
    };
    // Stays open while its contents are used.
    let config = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
    MenuButton::new(name(value)).config(config).ui(ui, |ui| {
        ui.set_width(170.0);
        let options = [
            (Shape::Uniform, "Every value is equally likely"),
            (Shape::Low(2.0), "Favors the first value"),
            (Shape::High(2.0), "Favors the second value"),
            (Shape::Middle(2.0), "Favors the middle"),
            (Shape::Falloff(4.0), "Mostly the first value, with a long thin tail"),
        ];
        for (option, hint) in options {
            let selected = std::mem::discriminant(value) == std::mem::discriminant(&option);
            if ui.selectable_label(selected, name(&option)).on_hover_text(hint).clicked()
                && !selected
            {
                // Keep the strength, as far as it's comparable.
                *value = match (*value, option) {
                    (
                        Shape::Low(power) | Shape::High(power) | Shape::Middle(power),
                        Shape::Low(_) | Shape::High(_) | Shape::Middle(_),
                    ) => match option {
                        Shape::Low(_) => Shape::Low(power),
                        Shape::High(_) => Shape::High(power),
                        _ => Shape::Middle(power),
                    },
                    _ => option,
                };
            }
        }
        if let Shape::Low(power)
        | Shape::High(power)
        | Shape::Middle(power)
        | Shape::Falloff(power) = value
        {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Strength");
                drag(ui, power, &Number::new(0.0, 10.0).at_least(0.01));
            });
        }
        distribution(ui, value);
    });
}

/// A histogram of a distribution.
fn distribution(ui: &mut Ui, shape: &Shape) {
    const BINS: usize = 32;
    const SAMPLES: usize = 1600;
    let mut counts = [0usize; BINS];
    for i in 0..SAMPLES {
        let t = shape.apply((i as f32 + 0.5) / SAMPLES as f32).clamp(0.0, 1.0);
        counts[((t * BINS as f32) as usize).min(BINS - 1)] += 1;
    }
    // The tallest bins of steep distributions are cut off, or the rest would hardly show.
    let full = (SAMPLES / BINS * 4) as f32;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let width = rect.width() / BINS as f32;
    for (i, count) in counts.into_iter().enumerate() {
        let height = rect.height() * (count as f32 / full).min(1.0);
        let left = rect.left() + width * i as f32;
        let bar = egui::Rect::from_min_max(
            egui::pos2(left, rect.bottom() - height),
            egui::pos2(left + width - 1.0, rect.bottom()),
        );
        painter.rect_filled(bar, 0.0, ui.visuals().selection.bg_fill);
    }
}

/// Splits a color into one a picker can show, and how much brighter than that it is.
pub fn split_intensity(color: [f32; 3]) -> ([f32; 3], f32) {
    let intensity = color.into_iter().fold(1.0, f32::max);
    (color.map(|c| (c / intensity).max(0.0)), intensity)
}

/// A linear color that can be brighter than white.
pub fn rgb(ui: &mut Ui, value: &mut Rgb) {
    let (mut base, mut intensity) = split_intensity([value.0, value.1, value.2]);
    let picked = ui.color_edit_button_rgb(&mut base).changed();
    ui.weak("×");
    let brightness = Number::new(1.0, 20.0).log().at_least(0.0);
    let scaled = drag(ui, &mut intensity, &brightness)
        .on_hover_text("Brightness; above 1 makes it glow")
        .changed();
    if picked || scaled {
        *value = Rgb(base[0] * intensity, base[1] * intensity, base[2] * intensity);
    }
}

/// An sRGB color made of one end of each channel's range.
pub fn srgb(ui: &mut Ui, channels: [&mut f32; 3]) {
    let (base, mut intensity) = split_intensity([*channels[0], *channels[1], *channels[2]]);
    // The picker's are linear.
    let linear = Color::srgb(base[0], base[1], base[2]).to_linear();
    let mut picker = [linear.red, linear.green, linear.blue];
    let picked = ui.color_edit_button_rgb(&mut picker).changed();
    ui.weak("×");
    let brightness = Number::new(1.0, 10.0).log().at_least(0.0);
    let scaled = drag(ui, &mut intensity, &brightness)
        .on_hover_text("Brightness; above 1 makes it glow")
        .changed();
    let base = if picked {
        let color = Color::linear_rgb(picker[0], picker[1], picker[2]).to_srgba();
        [color.red, color.green, color.blue]
    } else {
        base
    };
    if picked || scaled {
        for (channel, base) in channels.into_iter().zip(base) {
            *channel = base * intensity;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_positions_have_an_inverse() {
        for number in [
            Number::new(-5.0, 20.0),
            Number::new(0.0, 1e5).log(),
            Number::new(-1e7, 1e7).log(),
        ] {
            for i in 0..=10 {
                let position = i as f32 / 10.0;
                let back = number.position(number.at(position));
                assert!((back - position).abs() < 1e-3, "{position} became {back}");
            }
            assert_eq!((number.position(f32::MAX), number.position(f32::MIN)), (1.0, 0.0));
        }
    }

    #[test]
    fn stored_numbers_are_limited_and_finite() {
        let number = Number::angle(0.0, 360.0).at_least(0.0);
        let mut value = 1.0;
        number.store(&mut value, f32::NAN);
        assert_eq!(value, 1.0);
        number.store(&mut value, 180.0);
        assert_eq!(value, std::f32::consts::PI);
        number.store(&mut value, -90.0);
        assert_eq!(value, 0.0);
    }

    #[test]
    fn dragging_gives_tidy_numbers() {
        let number = Number::new(0.0, 100.0);
        assert_eq!(number.tidy(33.333_33, 0.333, 200.0), 33.3);
    }

    #[test]
    fn colors_split_into_a_base_and_an_intensity() {
        assert_eq!(split_intensity([0.5, 1.0, 4.0]), ([0.125, 0.25, 1.0], 4.0));
        assert_eq!(split_intensity([0.5, 0.0, 0.25]), ([0.5, 0.0, 0.25], 1.0));
        assert_eq!(split_intensity([0.0; 3]), ([0.0; 3], 1.0));
    }

    /// Runs `add` in a headless egui for some frames.
    fn show(mut add: impl FnMut(&mut Ui)) {
        let ctx = egui::Context::default();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| add(ui));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn showing_values_doesnt_change_them() {
        let mut spread_value = Spread::new(0.123_456_78..7e7, Shape::Low(1.234_567_8));
        let spread_before = spread_value.clone();
        let mut color = Rgb(0.123_456_7, 7.654_321, -0.5);
        let mut channels = [0.123_456_7, 1.765_432_1, 0.3];
        let mut angle = 1.234_567_8;
        let mut count = 16_777_217; // Not an `f32`.
        let mut position = Vec2::new(1e9, -0.000_012_3);
        show(|ui| {
            let number = Number::new(0.0, 1.0).at_least(0.5);
            spread_ends(ui, &mut spread_value, &number);
            spread_shape(ui, &mut spread_value, &number, true);
            rgb(ui, &mut color);
            srgb(ui, channels.each_mut());
            super::number(ui, "angle", "", &mut angle, &Number::angle(0.0, 10.0));
            integer(ui, "count", "", &mut count, &Number::new(0.0, 10.0));
            integer_on_release(ui, "count2", "", &mut count, &Number::new(1.0, 10.0));
            vec2(ui, &mut position, &Number::new(-1.0, 1.0));
        });
        assert_eq!(spread_value, spread_before);
        assert_eq!((color.0, color.1, color.2), (0.123_456_7, 7.654_321, -0.5));
        assert_eq!(channels, [0.123_456_7, 1.765_432_1, 0.3]);
        assert_eq!((angle, count), (1.234_567_8, 16_777_217));
        assert_eq!(position, Vec2::new(1e9, -0.000_012_3));
    }
}
