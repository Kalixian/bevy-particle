//! The keyframe editor: curves for numbers, pairs and vectors, gradients for colors.

use bevy::{math::Vec2, prelude::Color};
use bevy_egui::egui::{self, Color32, Rect, Response, Sense, Ui};
use bevy_particle::{Clock, Ease, Key, Keyable, Repeat, Rgb, Track};

use super::{
    theme,
    widgets::{self, Number},
};

const CURVE_HEIGHT: f32 = 120.0;
const STRIP_HEIGHT: f32 = 22.0;
const STOP_HEIGHT: f32 = 16.0;
/// How close to a key counts as being on it.
const GRAB_DISTANCE: f32 = 10.0;

/// Most components of a key: two colors.
const MAX_WIDTH: usize = 6;

/// A value that tracks of it can be edited.
pub trait Value: Keyable {
    fn load(self) -> [f32; MAX_WIDTH];
    fn store(components: [f32; MAX_WIDTH]) -> Self;
}

impl Value for f32 {
    fn load(self) -> [f32; MAX_WIDTH] {
        [self, 0.0, 0.0, 0.0, 0.0, 0.0]
    }

    fn store(components: [f32; MAX_WIDTH]) -> Self {
        components[0]
    }
}

impl Value for Vec2 {
    fn load(self) -> [f32; MAX_WIDTH] {
        [self.x, self.y, 0.0, 0.0, 0.0, 0.0]
    }

    fn store(components: [f32; MAX_WIDTH]) -> Self {
        Vec2::new(components[0], components[1])
    }
}

impl Value for Rgb {
    fn load(self) -> [f32; MAX_WIDTH] {
        [self.0, self.1, self.2, 0.0, 0.0, 0.0]
    }

    fn store(components: [f32; MAX_WIDTH]) -> Self {
        Rgb(components[0], components[1], components[2])
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct KeyData {
    time: f32,
    ease: Ease,
    values: [f32; MAX_WIDTH],
}

/// A track of any kind, as it's edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Keys {
    clock: Clock,
    repeat: Repeat,
    /// Sorted by time.
    keys: Vec<KeyData>,
}

impl Keys {
    pub fn of<T: Value>(track: &Track<T>) -> Self {
        let keys = track.keys().iter().map(|key| KeyData {
            time: key.time,
            ease: key.ease,
            values: key.value.load(),
        });
        Self { clock: track.clock, repeat: track.repeat, keys: keys.collect() }
    }

    pub fn store<T: Value>(&self, track: &mut Track<T>) {
        track.clock = self.clock;
        track.repeat = self.repeat;
        track.set_keys(self.map_keys(T::store));
    }

    fn map_keys<T>(&self, value: impl Fn([f32; MAX_WIDTH]) -> T) -> Vec<Key<T>> {
        let key = |key: &KeyData| Key { time: key.time, value: value(key.values), ease: key.ease };
        self.keys.iter().map(key).collect()
    }

    /// Two colors, from tracks of the range of each channel. `None` unless their keys are at the
    /// same times.
    pub fn of_channels(channels: [&Track<Vec2>; 3]) -> Option<Self> {
        let [mut red, green, blue] = channels.map(Self::of);
        for (channel, other) in [&green, &blue].into_iter().enumerate() {
            let same = red.clock == other.clock
                && red.repeat == other.repeat
                && red.keys.len() == other.keys.len();
            if !same {
                return None;
            }
            for (key, other) in red.keys.iter_mut().zip(&other.keys) {
                if (key.time, key.ease) != (other.time, other.ease) {
                    return None;
                }
                let first = 2 * (channel + 1);
                key.values[first..first + 2].copy_from_slice(&other.values[..2]);
            }
        }
        Some(red)
    }

    /// The inverse of [`Self::of_channels`].
    pub fn store_channels(&self, channels: [&mut Track<Vec2>; 3]) {
        for (channel, track) in channels.into_iter().enumerate() {
            track.clock = self.clock;
            track.repeat = self.repeat;
            let first = 2 * channel;
            track.set_keys(self.map_keys(|values| Vec2::new(values[first], values[first + 1])));
        }
    }

    /// The values at `time`, which is within the track's first run. Must match `Track::sample`.
    fn sample(&self, time: f32) -> [f32; MAX_WIDTH] {
        let next = self.keys.partition_point(|key| key.time <= time);
        let Some(from) = next.checked_sub(1).map(|index| &self.keys[index]) else {
            return self.keys.first().map_or([0.0; MAX_WIDTH], |key| key.values);
        };
        let Some(to) = self.keys.get(next) else {
            return from.values;
        };
        let x = (time - from.time) / (to.time - from.time);
        let x = match from.ease {
            Ease::Linear => x,
            Ease::Smooth => x * x * (3.0 - 2.0 * x),
            Ease::Step => 0.0,
        };
        std::array::from_fn(|i| from.values[i] + (to.values[i] - from.values[i]) * x)
    }

    /// Where in its first run a track is at a time of its clock.
    fn wrap(&self, time: f32) -> f32 {
        let period = self.keys.last().map_or(0.0, |key| key.time);
        match self.repeat {
            Repeat::Loop if period > 0.0 => time.rem_euclid(period),
            Repeat::PingPong if period > 0.0 => {
                period - (time.rem_euclid(2.0 * period) - period).abs()
            }
            _ => time,
        }
    }

    /// Returns where the key at `index` is afterwards.
    fn set_time(&mut self, index: usize, time: f32) -> usize {
        let mut key = self.keys.remove(index);
        key.time = if self.clock == Clock::Life { time.clamp(0.0, 1.0) } else { time.max(0.0) };
        // Stay on the same side of keys at the same time.
        let mut place = self.keys.partition_point(|other| other.time < key.time);
        while place < index && self.keys.get(place).is_some_and(|other| other.time == key.time) {
            place += 1;
        }
        self.keys.insert(place, key);
        place
    }

    /// Adds a key that's on the curve, and returns where it is.
    fn insert(&mut self, time: f32) -> usize {
        let place = self.keys.partition_point(|key| key.time <= time);
        let ease = place.checked_sub(1).map_or(Ease::Linear, |index| self.keys[index].ease);
        let values = self.sample(time);
        self.keys.insert(place, KeyData { time, ease, values });
        self.set_time(place, time)
    }

    /// Tracks need at least one key to do anything.
    fn can_remove(&self) -> bool {
        self.keys.len() > 1
    }
}

/// How a track's components are shown.
#[derive(Clone)]
pub enum Look {
    /// A curve for each name.
    Curves(&'static [&'static str], Number),
    /// A linear color.
    Color,
    /// Two sRGB colors, as made by [`Keys::of_channels`].
    Colors,
}

impl Look {
    fn curve_count(&self) -> usize {
        match self {
            Self::Curves(names, _) => names.len(),
            _ => 0,
        }
    }

    fn strip_count(&self) -> usize {
        match self {
            Self::Curves(..) => 0,
            Self::Color => 1,
            Self::Colors => 2,
        }
    }

    /// What a strip looks like where the track has these values. Colors brighter than white are
    /// scaled down to it.
    fn color(&self, strip: usize, values: &[f32; MAX_WIDTH]) -> Color32 {
        let color = match self {
            Self::Colors => [values[strip], values[2 + strip], values[4 + strip]],
            _ => [values[0], values[1], values[2]],
        };
        let (base, _) = widgets::split_intensity(color);
        let srgb = match self {
            Self::Colors => Color::srgb(base[0], base[1], base[2]).to_srgba(),
            _ => Color::linear_rgb(base[0], base[1], base[2]).to_srgba(),
        };
        let byte = |c: f32| (c * 255.0).round() as u8;
        Color32::from_rgb(byte(srgb.red), byte(srgb.green), byte(srgb.blue))
    }
}

const CURVE_COLORS: [Color32; 2] =
    [Color32::from_rgb(110, 190, 255), Color32::from_rgb(255, 170, 90)];

/// What surrounds a track.
#[derive(Clone, Copy, Default)]
pub struct Context<'a> {
    /// Whether only `Clock::Time` works.
    pub time_only: bool,
    /// The age of the owner, while it's on screen.
    pub age: Option<f32>,
    /// What to say if the track is over the life of particles.
    pub life_warning: Option<&'a str>,
}

/// What part of a track is visible: from 0 to 1 in a rectangle is from `min` to `max`.
#[derive(Clone, Copy, Debug)]
struct View {
    rect: Rect,
    /// Time, value.
    min: Vec2,
    max: Vec2,
}

impl View {
    /// Shows everything. Values are in shown units.
    fn fit(keys: &Keys, look: &Look, rect: Rect) -> Self {
        let first = keys.keys.first().map_or(0.0, |key| key.time);
        let last = keys.keys.last().map_or(0.0, |key| key.time);
        let end = match keys.clock {
            Clock::Life => 1.0,
            // With room to drag the last key to the right.
            _ if last > 0.0 => last * 1.1,
            _ => 1.0,
        };
        let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut pad = 0.5;
        if let Look::Curves(names, number) = look {
            for value in keys.keys.iter().flat_map(|key| &key.values[..names.len()]) {
                min = min.min(value * number.scale());
                max = max.max(value * number.scale());
            }
            pad = number.span() * 0.05;
        }
        if !(min.is_finite() && max.is_finite()) {
            (min, max) = (0.0, 1.0);
        }
        // Flat curves are in the middle.
        pad = if max > min { (max - min) * 0.15 } else { (max.abs() * 0.5).max(pad) };
        Self {
            rect,
            min: Vec2::new(first.min(0.0), min - pad),
            max: Vec2::new(end.max(last), max + pad),
        }
    }

    fn x(&self, time: f32) -> f32 {
        let position = (time - self.min.x) / (self.max.x - self.min.x);
        self.rect.left() + self.rect.width() * position
    }

    fn y(&self, shown: f32) -> f32 {
        let position = (shown - self.min.y) / (self.max.y - self.min.y);
        self.rect.bottom() - self.rect.height() * position
    }

    fn time(&self, x: f32) -> f32 {
        let per_point = (self.max.x - self.min.x) / self.rect.width();
        widgets::round_to_step(self.min.x + (x - self.rect.left()) * per_point, per_point)
    }

    fn shown(&self, y: f32) -> f32 {
        let per_point = (self.max.y - self.min.y) / self.rect.height();
        widgets::round_to_step(self.min.y + (self.rect.bottom() - y) * per_point, per_point)
    }
}

/// What the editor of a track remembers.
#[derive(Clone, Copy, Default)]
struct State {
    selected: usize,
    /// The key and component being dragged.
    drag: Option<(usize, usize)>,
    /// Kept while dragging, or the curve would run away from the pointer.
    view: Option<View>,
}

/// A small picture of a track. Clicking it opens and closes the editor.
pub fn preview(ui: &mut Ui, keys: &Keys, look: &Look, open: &mut bool) {
    let size = egui::vec2(ui.available_width().max(40.0), ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *open = !*open;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let view = View::fit(keys, look, rect.shrink2(egui::vec2(4.0, 2.0)));
    draw_strips(&painter, keys, look, &view, rect.shrink(2.0));
    draw_curves(&painter, keys, look, &view, 1.0);
    if response.hovered() || *open {
        let stroke = ui.style().interact(&response).fg_stroke;
        painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
    }
    let count = keys.keys.len();
    let hint = format!("{count} keyframe{}; click to edit", if count == 1 { "" } else { "s" });
    response.on_hover_text(hint);
}

fn draw_curves(painter: &egui::Painter, keys: &Keys, look: &Look, view: &View, width: f32) {
    let Look::Curves(names, number) = look else {
        return;
    };
    for component in 0..names.len() {
        let y = |values: &[f32; MAX_WIDTH]| view.y(values[component] * number.scale());
        let mut points = Vec::new();
        let count = view.rect.width().max(2.0) as usize / 2;
        let mut times: Vec<_> = (0..=count)
            .map(|i| view.min.x + (view.max.x - view.min.x) * i as f32 / count as f32)
            .collect();
        times.extend(keys.keys.iter().map(|key| key.time));
        times.sort_by(f32::total_cmp);
        let mut next_key = keys.keys.iter().peekable();
        for time in times {
            // Cuts and steps are vertical: draw up to the key from before it.
            while let Some(key) = next_key.next_if(|key| key.time <= time) {
                let before = keys.keys.iter().rposition(|other| other.time < key.time);
                let before = match before.map(|index| &keys.keys[index]) {
                    Some(from) if from.ease == Ease::Step => from.values,
                    Some(_) => {
                        let first = keys.keys.iter().find(|other| other.time == key.time);
                        first.map_or(key.values, |key| key.values)
                    }
                    None => key.values,
                };
                points.push(egui::pos2(view.x(key.time), y(&before)));
            }
            points.push(egui::pos2(view.x(time), y(&keys.sample(time))));
        }
        let stroke = egui::Stroke::new(width, CURVE_COLORS[component % CURVE_COLORS.len()]);
        painter.add(egui::Shape::line(points, stroke));
    }
}

fn draw_strips(painter: &egui::Painter, keys: &Keys, look: &Look, view: &View, rect: Rect) {
    let strips = look.strip_count();
    let height = rect.height() / strips.max(1) as f32;
    for strip in 0..strips {
        let top = rect.top() + height * strip as f32;
        let columns = (rect.width() / 2.0).ceil().max(1.0) as usize;
        for column in 0..columns {
            let left = rect.left() + column as f32 * 2.0;
            let per_point = (view.max.x - view.min.x) / view.rect.width();
            let time = view.min.x + (left + 1.0 - view.rect.left()) * per_point;
            let cell = Rect::from_min_max(
                egui::pos2(left, top),
                egui::pos2((left + 2.0).min(rect.right()), top + height),
            );
            painter.rect_filled(cell, 0.0, look.color(strip, &keys.sample(time)));
        }
    }
}

/// The editor of a track.
pub fn editor(ui: &mut Ui, keys: &mut Keys, look: &Look, context: Context) {
    let id = ui.id().with("keys");
    let mut state: State = ui.data(|data| data.get_temp(id)).unwrap_or_default();
    state.selected = state.selected.min(keys.keys.len().saturating_sub(1));

    let response = canvas(ui, keys, look, context, &mut state);
    let delete = |input: &egui::InputState| {
        input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
    };
    if response.hovered() && !ui.ctx().egui_wants_keyboard_input() && ui.input(delete) {
        remove_selected(keys, &mut state);
    }

    ui.horizontal(|ui| {
        if !context.time_only {
            let options = [
                (Clock::Life, "Over life"),
                (Clock::Seconds, "Particle age"),
                (Clock::Time, "Scene time"),
            ];
            let before = keys.clock;
            widgets::choice(ui, "clock", &mut keys.clock, &options);
            if keys.clock != before && keys.clock == Clock::Life {
                for key in &mut keys.keys {
                    key.time = key.time.clamp(0.0, 1.0);
                }
            }
        }
        // It's ignored for tracks over life.
        if keys.clock != Clock::Life {
            let options =
                [(Repeat::Once, "Once"), (Repeat::Loop, "Loop"), (Repeat::PingPong, "Ping-pong")];
            widgets::choice(ui, "repeat", &mut keys.repeat, &options);
        }
    });
    if keys.clock == Clock::Life
        && let Some(warning) = context.life_warning
    {
        ui.colored_label(theme::WARNING, warning);
    }
    show_selected(ui, keys, look, &mut state);
    ui.data_mut(|data| data.insert_temp(id, state));
}

/// Curves or gradients, with keys to drag.
fn canvas(
    ui: &mut Ui,
    keys: &mut Keys,
    look: &Look,
    context: Context,
    state: &mut State,
) -> Response {
    let strips = look.strip_count();
    let height =
        if strips > 0 { STRIP_HEIGHT * strips as f32 + STOP_HEIGHT } else { CURVE_HEIGHT };
    let size = egui::vec2(ui.available_width().max(60.0), height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let area = rect.shrink2(egui::vec2(8.0, if strips > 0 { 0.0 } else { 8.0 }));
    let fit = View::fit(keys, look, area);
    // Follow the layout, but not the keys.
    let view = state.view.map_or(fit, |view| View { rect: area, ..view });

    let selected = state.selected;
    // The closest key to a point, and its component.
    let key_at = |keys: &Keys, point: egui::Pos2| {
        let mut best = None;
        let mut best_distance = GRAB_DISTANCE;
        for (index, key) in keys.keys.iter().enumerate() {
            let x = view.x(key.time);
            for component in 0..look.curve_count().max(1) {
                let distance = match look {
                    Look::Curves(_, number) => {
                        let y = view.y(key.values[component] * number.scale());
                        point.distance(egui::pos2(x, y))
                    }
                    _ => (point.x - x).abs(),
                };
                // Of keys in the same place, the selected one; of its components, the last one,
                // which is drawn on top.
                let same_key = best.is_some_and(|(other, _)| other == index);
                let preferred =
                    distance == best_distance && (same_key || index == selected);
                if distance < best_distance || preferred {
                    (best, best_distance) = (Some((index, component)), distance);
                }
            }
        }
        best
    };

    let origin = ui.input(|input| input.pointer.press_origin());
    if response.drag_started()
        && let Some(hit) = origin.and_then(|origin| key_at(keys, origin))
    {
        state.drag = Some(hit);
        state.view = Some(fit);
        state.selected = hit.0;
    }
    if let Some((index, component)) = state.drag
        && response.dragged()
        && response.drag_delta() != egui::Vec2::ZERO
        && let (Some(pointer), Some(key)) =
            (response.interact_pointer_pos(), keys.keys.get_mut(index))
    {
        if let Look::Curves(_, number) = look {
            number.store(&mut key.values[component], view.shown(pointer.y));
        }
        let index = keys.set_time(index, view.time(pointer.x));
        state.drag = Some((index, component));
        state.selected = index;
    }
    if !response.dragged() && !response.is_pointer_button_down_on() {
        state.drag = None;
        state.view = None;
    }
    if let Some(pointer) = response.interact_pointer_pos() {
        match key_at(keys, pointer) {
            Some((index, _)) if response.clicked() => state.selected = index,
            None if response.double_clicked() => {
                state.selected = keys.insert(view.time(pointer.x));
            }
            _ => {}
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let strip_rect = Rect::from_min_max(
        area.min,
        egui::pos2(area.right(), area.top() + STRIP_HEIGHT * strips as f32),
    );
    draw_strips(&painter, keys, look, &view, strip_rect);
    draw_axes(ui, &painter, rect, &view, look, keys.clock);
    draw_curves(&painter, keys, look, &view, 1.5);
    if keys.clock == Clock::Time
        && let Some(age) = context.age
    {
        let x = view.x(keys.wrap(age)).clamp(area.left(), area.right());
        painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, theme::WARNING));
    }
    for (index, key) in keys.keys.iter().enumerate() {
        let x = view.x(key.time);
        let selected = index == state.selected;
        let outline = if selected { Color32::WHITE } else { Color32::from_gray(20) };
        match look {
            Look::Curves(names, number) => {
                for component in 0..names.len() {
                    let y = view.y(key.values[component] * number.scale());
                    let color = CURVE_COLORS[component % CURVE_COLORS.len()];
                    diamond(&painter, egui::pos2(x, y), 5.5, color, outline);
                }
            }
            _ => {
                let center = egui::pos2(x, strip_rect.bottom() + STOP_HEIGHT / 2.0);
                diamond(&painter, center, 6.0, look.color(0, &key.values), outline);
                if !selected {
                    let line = egui::Stroke::new(1.0, Color32::from_gray(120));
                    painter.add(egui::Shape::closed_line(diamond_points(center, 6.0), line));
                }
            }
        }
    }
    // Only once the pointer rests, so as not to get in the way.
    let still = ui.input(|input| input.pointer.time_since_last_movement() > 1.0);
    if still && !response.is_pointer_button_down_on() {
        response.on_hover_text("Double-click to add a key; Delete removes it")
    } else {
        response
    }
}

fn draw_axes(
    ui: &Ui,
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    look: &Look,
    clock: Clock,
) {
    let font = egui::FontId::proportional(10.0);
    let color = ui.visuals().weak_text_color();
    let Look::Curves(_, number) = look else {
        return;
    };
    let line = egui::Stroke::new(1.0, Color32::from_gray(45));
    if view.min.y < 0.0 && view.max.y > 0.0 {
        painter.hline(rect.x_range(), view.y(0.0), line);
    }
    for i in 1..4 {
        let time = view.min.x + (view.max.x - view.min.x) * i as f32 / 4.0;
        painter.vline(view.x(time), rect.y_range(), line);
    }
    let text = |position, align, text: String| {
        painter.text(position, align, text, font.clone(), color);
    };
    let value = |value: f32| format!("{}{}", short(value), number.suffix_text());
    let corner = rect.shrink(3.0);
    text(corner.left_top(), egui::Align2::LEFT_TOP, value(view.max.y));
    text(corner.left_bottom(), egui::Align2::LEFT_BOTTOM, value(view.min.y));
    let unit = if clock == Clock::Life { " of life" } else { " s" };
    let end = format!("{}{unit}", short(view.max.x));
    text(corner.right_bottom(), egui::Align2::RIGHT_BOTTOM, end);
}

/// A number with about three digits.
fn short(value: f32) -> String {
    let decimals = (2.0 - value.abs().max(1e-6).log10().floor()).clamp(0.0, 3.0) as usize;
    let text = format!("{value:.decimals$}");
    if decimals == 0 {
        return text;
    }
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn diamond_points(center: egui::Pos2, radius: f32) -> Vec<egui::Pos2> {
    [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]
        .map(|(x, y)| center + egui::vec2(x, y) * radius)
        .to_vec()
}

fn diamond(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    fill: Color32,
    outline: Color32,
) {
    let stroke = egui::Stroke::new(1.5, outline);
    let points = diamond_points(center, radius);
    painter.add(egui::Shape::convex_polygon(points, fill, stroke));
}

fn remove_selected(keys: &mut Keys, state: &mut State) {
    if keys.can_remove() {
        keys.keys.remove(state.selected);
        state.selected = state.selected.min(keys.keys.len() - 1);
        state.drag = None;
    }
}

/// Exact values of the selected key.
fn show_selected(ui: &mut Ui, keys: &mut Keys, look: &Look, state: &mut State) {
    let count = keys.keys.len();
    if count == 0 {
        if ui.button("Add key").clicked() {
            keys.insert(0.0);
        }
        return;
    }
    let mut remove = false;
    ui.horizontal(|ui| {
        // Also reaches keys that are hidden by others.
        if ui.add_enabled(state.selected > 0, egui::Button::new("⏴").small()).clicked() {
            state.selected -= 1;
        }
        ui.label(format!("Key {} of {count}", state.selected + 1));
        let next = ui.add_enabled(state.selected + 1 < count, egui::Button::new("⏵").small());
        if next.clicked() {
            state.selected += 1;
        }
        let button = ui.add_enabled(keys.can_remove(), egui::Button::new("Remove").small());
        remove = button.clicked();
        if ui.small_button("Add").on_hover_text("Adds a key after this one").clicked() {
            let time = keys.keys[state.selected].time;
            let next = keys.keys.get(state.selected + 1).map(|key| key.time);
            let end = if keys.clock == Clock::Life { 1.0 } else { time + 1.0 };
            state.selected = keys.insert((time + next.unwrap_or(end)) / 2.0);
        }
    });
    let mut index = state.selected;
    widgets::row(ui, "Time", "", |ui| {
        let mut time = keys.keys[index].time;
        let number = match keys.clock {
            Clock::Life => Number::new(0.0, 1.0).at_least(0.0).at_most(1.0),
            _ => Number::new(0.0, 10.0).at_least(0.0).suffix(" s"),
        };
        if widgets::drag(ui, &mut time, &number).changed() {
            index = keys.set_time(index, time);
        }
        let key = &mut keys.keys[index];
        let options =
            [(Ease::Linear, "Linear"), (Ease::Smooth, "Smooth"), (Ease::Step, "Step")];
        // How it gets from this key to the next.
        widgets::choice(ui, "ease", &mut key.ease, &options);
    });
    state.selected = index;
    let key = &mut keys.keys[index];
    match look {
        Look::Curves([name], number) => {
            widgets::row(ui, name, "", |ui| widgets::drag(ui, &mut key.values[0], number));
        }
        Look::Curves(names, number) => {
            widgets::row(ui, "Values", "", |ui| {
                for (component, name) in names.iter().enumerate() {
                    ui.colored_label(CURVE_COLORS[component % CURVE_COLORS.len()], *name);
                    widgets::drag(ui, &mut key.values[component], number);
                }
            });
        }
        Look::Color => {
            widgets::row(ui, "Color", "", |ui| {
                let mut color = Rgb::store(key.values);
                widgets::rgb(ui, &mut color);
                key.values = color.load();
            });
        }
        Look::Colors => {
            let [r0, r1, g0, g1, b0, b1] = &mut key.values;
            widgets::row(ui, "From", "", |ui| widgets::srgb(ui, [r0, g0, b0]));
            widgets::row(ui, "To", "", |ui| widgets::srgb(ui, [r1, g1, b1]));
        }
    }
    if remove {
        remove_selected(keys, state);
    }
}

/// A diamond that turns keyframes on (filled) and off. Returns whether it was clicked.
pub fn key_button(ui: &mut Ui, keyed: bool) -> bool {
    let size = egui::vec2(18.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let label = if keyed { "Remove the keyframes" } else { "Animate with keyframes" };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, keyed, label)
    });
    let color = ui.style().interact(&response).fg_stroke.color;
    let (fill, outline) = if keyed {
        (theme::KEYED, if response.hovered() { color } else { theme::KEYED })
    } else {
        (Color32::TRANSPARENT, color)
    };
    diamond(ui.painter(), rect.center(), 5.5, fill, outline);
    response.on_hover_text(label).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(clock: Clock, points: &[(f32, f32, Ease)]) -> Keys {
        let keys = points.iter().map(|&(time, value, ease)| KeyData {
            time,
            ease,
            values: value.load(),
        });
        Keys { clock, repeat: Repeat::Once, keys: keys.collect() }
    }

    fn times(keys: &Keys) -> Vec<f32> {
        keys.keys.iter().map(|key| key.time).collect()
    }

    #[test]
    fn tracks_survive_being_edited() {
        let mut track =
            Track::over_time([(0.0, Vec2::X, Ease::Smooth), (2.0, Vec2::Y, Ease::Step)]).looped();
        let before = track.clone();
        Keys::of(&before).store(&mut track);
        assert_eq!(track, before);
        let mut color = Track::over_life([(0.5, Rgb(1.0, 2.0, 3.0))]);
        let before = color.clone();
        Keys::of(&before).store(&mut color);
        assert_eq!(color, before);
    }

    #[test]
    fn values_match_the_track() {
        let track = Track::over_time([
            (0.5, 1.0, Ease::Smooth),
            (1.0, 3.0, Ease::Linear),
            (1.0, 8.0, Ease::Step),
            (2.0, -4.0, Ease::Linear),
        ]);
        for track in [track.clone(), track.clone().looped(), track.ping_pong()] {
            let keys = Keys::of(&track);
            for i in 0..100 {
                let time = i as f32 * 0.07;
                let expected = track.sample(time).unwrap().x;
                let value = keys.sample(keys.wrap(time))[0];
                assert!((value - expected).abs() < 1e-4, "{value} != {expected} at {time}");
            }
        }
    }

    #[test]
    fn color_channels_are_joined_if_their_keys_match() {
        let track = |offset: f32| {
            let value = |x| Vec2::new(x, x + 0.5) + offset;
            Track::over_time([(0.0, value(1.0)), (1.0, value(2.0))])
        };
        let mut tracks = [track(0.0), track(10.0), track(20.0)];
        let before = tracks.clone();
        let keys = Keys::of_channels(tracks.each_ref()).unwrap();
        assert_eq!(keys.keys[1].values, [2.0, 2.5, 12.0, 12.5, 22.0, 22.5]);
        keys.store_channels(tracks.each_mut());
        assert_eq!(tracks, before);

        tracks[2] = Track::over_time([(0.0, Vec2::X), (1.5, Vec2::X)]);
        assert!(Keys::of_channels(tracks.each_ref()).is_none());
        tracks[2] = track(0.0).looped();
        assert!(Keys::of_channels(tracks.each_ref()).is_none());
    }

    #[test]
    fn keys_stay_sorted_when_they_move() {
        let linear = Ease::Linear;
        let points = [(0.0, 0.0, linear), (1.0, 1.0, linear), (2.0, 2.0, linear)];
        let mut moved = keys(Clock::Time, &points);
        assert_eq!(moved.set_time(0, 1.5), 1);
        assert_eq!(times(&moved), [1.0, 1.5, 2.0]);
        assert_eq!(moved.keys[1].values[0], 0.0);
        assert_eq!(moved.set_time(2, -3.0), 0);
        assert_eq!(times(&moved), [0.0, 1.0, 1.5]);

        // Both ways of moving onto another key's time keep the order.
        let mut cut = keys(Clock::Time, &points);
        assert_eq!(cut.set_time(1, 2.0), 1);
        let mut cut = keys(Clock::Time, &points);
        assert_eq!(cut.set_time(1, 0.0), 1);

        let mut life = keys(Clock::Life, &points[..2]);
        assert_eq!(life.set_time(0, 7.0), 0);
        assert_eq!(times(&life), [1.0, 1.0]);
    }

    #[test]
    fn added_keys_are_on_the_curve() {
        let mut keys =
            keys(Clock::Time, &[(0.0, 0.0, Ease::Linear), (2.0, 4.0, Ease::Linear)]);
        assert_eq!(keys.insert(1.0), 1);
        assert_eq!(keys.keys[1].values[0], 2.0);
        assert_eq!(keys.insert(5.0), 3);
        assert_eq!(keys.keys[3].values[0], 4.0);
        let mut empty = Keys { keys: Vec::new(), ..keys };
        assert_eq!(empty.insert(0.5), 0);
    }

    #[test]
    fn the_last_key_stays() {
        let mut keys = keys(Clock::Life, &[(0.0, 1.0, Ease::Linear), (1.0, 2.0, Ease::Linear)]);
        let mut state = State { selected: 1, ..Default::default() };
        remove_selected(&mut keys, &mut state);
        remove_selected(&mut keys, &mut state);
        assert_eq!((times(&keys), state.selected), (vec![0.0], 0));
    }

    #[test]
    fn views_have_an_inverse_and_show_every_key() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0));
        let look = Look::Curves(&["Value"], Number::new(0.0, 10.0));
        for points in [
            &[(0.0, 5.0, Ease::Linear), (3.0, -5.0, Ease::Linear)][..],
            &[(0.0, 7.0, Ease::Linear)],
            &[],
        ] {
            let keys = keys(Clock::Time, points);
            let view = View::fit(&keys, &look, rect);
            for &(time, value, _) in points {
                let point = egui::pos2(view.x(time), view.y(value));
                assert!(rect.contains(point), "{point:?}");
                assert!((view.time(point.x) - time).abs() < 0.03);
                assert!((view.shown(point.y) - value).abs() < 0.2);
            }
        }
    }

    #[test]
    fn short_numbers() {
        let cases = [(0.0, "0"), (1.5, "1.5"), (0.0626, "0.063"), (12345.6, "12346"), (100.0, "100")];
        for (value, text) in cases {
            assert_eq!(short(value), text);
        }
    }
}
