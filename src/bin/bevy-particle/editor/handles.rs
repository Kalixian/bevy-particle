//! Handles in the scene, for moving and sizing what's selected with the mouse.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Sense};
use bevy_particle::{
    Animated, EffectFile, EmitterDef, EmitterEntry, EmitterSource, Modifier, SpawnArea, Track,
};

use super::{
    documents::Documents,
    inspector::{emitter_path, Ages, Selection},
    theme, widgets,
};

const HANDLE_SIZE: f32 = 18.0;
/// How far the rotation handle is from its emitter, in pixels.
const ROTATION_DISTANCE: f32 = 90.0;
const OUTLINE: Color32 = Color32::from_rgb(150, 150, 160);

/// What a handle changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Target {
    Position,
    PositionKey(usize),
    Rotation,
    /// Both half extents.
    Corner,
    RadiusX,
    RadiusY,
    RingInner,
    RingOuter,
    LineFrom,
    LineTo,
    /// Of the modifier at this index.
    Offset(usize),
    OffsetKey(usize, usize),
    Bounds(usize),
}

#[derive(Debug, Clone)]
struct Handle {
    target: Target,
    label: String,
    /// In the world.
    at: Vec2,
}

/// Lines through some points in the world.
#[derive(Debug, Clone)]
struct Outline {
    points: Vec<Vec2>,
    closed: bool,
}

/// What's drawn for the selection.
#[derive(Debug, Default)]
struct Overlay {
    handles: Vec<Handle>,
    outlines: Vec<Outline>,
}

impl Overlay {
    fn handle(&mut self, target: Target, label: impl Into<String>, at: Vec2) {
        self.handles.push(Handle { target, label: label.into(), at });
    }

    fn line(&mut self, points: impl IntoIterator<Item = Vec2>) {
        self.outlines.push(Outline { points: points.into_iter().collect(), closed: false });
    }

    fn shape(&mut self, points: impl IntoIterator<Item = Vec2>) {
        self.outlines.push(Outline { points: points.into_iter().collect(), closed: true });
    }
}

/// Where an emitter is.
#[derive(Clone, Copy)]
struct Frame {
    origin: Vec2,
    rotation: f32,
}

impl Frame {
    fn new(entry: &EmitterEntry, age: f32) -> Self {
        // With other clocks or no keys, tracks are ignored.
        let position = entry.motion.position.as_ref().and_then(|track| sample(track, age));
        let rotation = entry.motion.rotation.as_ref().and_then(|track| sample(track, age));
        Self {
            origin: position.map_or(entry.position, |value| value.truncate().truncate()),
            rotation: rotation.map_or(entry.rotation, |value| value.x),
        }
    }

    fn to_world(self, local: Vec2) -> Vec2 {
        self.origin + Vec2::from_angle(self.rotation).rotate(local)
    }

    fn to_local(self, world: Vec2) -> Vec2 {
        Vec2::from_angle(-self.rotation).rotate(world - self.origin)
    }
}

fn sample<T: bevy_particle::Keyable>(track: &Track<T>, age: f32) -> Option<Vec4> {
    if track.clock == bevy_particle::Clock::Time { track.sample(age) } else { None }
}

/// `pixel` is how large a pixel is in the world.
fn emitter_overlay(entry: &EmitterEntry, def: &EmitterDef, age: f32, pixel: f32) -> Overlay {
    let mut overlay = Overlay::default();
    let frame = Frame::new(entry, age);

    let keyed_area = def.tracks.as_ref().is_some_and(|tracks| tracks.spawn.is_some());
    let emitter = match &def.tracks {
        Some(tracks) => tracks.apply(&def.emitter, age),
        None => def.emitter.clone(),
    };
    area_overlay(&mut overlay, &emitter.spawn, frame, !keyed_area);
    modifiers_overlay(&mut overlay, &def.modifiers, frame.origin);

    let tip = frame.to_world(Vec2::new(ROTATION_DISTANCE * pixel, 0.0));
    overlay.line([frame.origin, tip]);
    if entry.motion.rotation.is_none() {
        overlay.handle(Target::Rotation, "Rotation", tip);
    }
    // Last, to be on top.
    match &entry.motion.position {
        None => overlay.handle(Target::Position, "Position", frame.origin),
        Some(track) => {
            let path: Vec<_> = track.keys().iter().map(|key| key.value).collect();
            for (index, at) in path.iter().enumerate() {
                let label = format!("Position, key {}", index + 1);
                overlay.handle(Target::PositionKey(index), label, *at);
            }
            overlay.line(path);
        }
    }
    overlay
}

fn ellipse(radii: Vec2) -> impl Iterator<Item = Vec2> {
    (0..64).map(move |i| Vec2::from_angle(i as f32 / 64.0 * TAU) * radii)
}

fn rectangle(half: Vec2) -> [Vec2; 4] {
    [half, Vec2::new(-half.x, half.y), -half, Vec2::new(half.x, -half.y)]
}

fn area_overlay(overlay: &mut Overlay, area: &SpawnArea, frame: Frame, editable: bool) {
    let world = |local| frame.to_world(local);
    match area {
        SpawnArea::Point => {}
        SpawnArea::Rect { half_extents, .. } => {
            overlay.shape(rectangle(*half_extents).map(world));
            if editable {
                overlay.handle(Target::Corner, "Half size", world(half_extents.abs()));
            }
        }
        SpawnArea::Ellipse { radii, .. } => {
            overlay.shape(ellipse(*radii).map(world));
            if editable {
                let radii = radii.abs();
                overlay.handle(Target::RadiusX, "Radius x", world(Vec2::new(radii.x, 0.0)));
                overlay.handle(Target::RadiusY, "Radius y", world(Vec2::new(0.0, radii.y)));
            }
        }
        SpawnArea::Ring { radius, arc } => {
            for radius in [radius.range.start, radius.range.end] {
                // The arc can be larger than a turn.
                let points = (0..=64).map(|i| {
                    let t = i as f32 / 64.0;
                    let angle = arc.range.start + (arc.range.end - arc.range.start) * t;
                    world(Vec2::from_angle(angle) * radius)
                });
                overlay.line(points);
            }
            if editable {
                // On the arc, but away from the rotation handle.
                let direction = Vec2::from_angle(ring_handle_angle(arc));
                let at = |radius: f32| world(direction * radius);
                overlay.handle(Target::RingInner, "Ring radius, first", at(radius.range.start));
                overlay.handle(Target::RingOuter, "Ring radius, second", at(radius.range.end));
            }
        }
        SpawnArea::Line { from, to } => {
            overlay.line([world(*from), world(*to)]);
            if editable {
                overlay.handle(Target::LineFrom, "From", world(*from));
                overlay.handle(Target::LineTo, "To", world(*to));
            }
        }
    }
}

fn ring_handle_angle(arc: &bevy_particle::Spread) -> f32 {
    arc.range.start + (arc.range.end - arc.range.start) * 0.25
}

/// `origin` is that of the modifiers' owner.
fn modifiers_overlay(overlay: &mut Overlay, modifiers: &[Modifier], origin: Vec2) {
    for (index, modifier) in modifiers.iter().enumerate() {
        match modifier {
            Modifier::PointAttractor { offset, .. } | Modifier::Vortex { offset, .. } => {
                let name = match modifier {
                    Modifier::Vortex { .. } => "Vortex",
                    _ => "Point attractor",
                };
                match offset {
                    Animated::Constant(offset) => {
                        overlay.handle(Target::Offset(index), name, origin + *offset);
                    }
                    Animated::Track(track) => {
                        let path: Vec<_> =
                            track.keys().iter().map(|key| origin + key.value).collect();
                        for (key, at) in path.iter().enumerate() {
                            let label = format!("{name}, key {}", key + 1);
                            overlay.handle(Target::OffsetKey(index, key), label, *at);
                        }
                        overlay.line(path);
                    }
                }
            }
            Modifier::Bounds { half_extents: Animated::Constant(half), .. } => {
                overlay.shape(rectangle(*half).map(|corner| origin + corner));
                overlay.handle(Target::Bounds(index), "Bounds", origin + half.abs());
            }
            _ => {}
        }
    }
}

/// Moves an emitter's handle.
fn apply_emitter(
    target: Target,
    world: Vec2,
    entry: &mut EmitterEntry,
    def: &mut EmitterDef,
    age: f32,
    pixel: f32,
) {
    let frame = Frame::new(entry, age);
    let local = frame.to_local(world);
    let tidy = |value: Vec2| round(value, pixel);
    match (target, &mut def.emitter.spawn) {
        (Target::Position, _) => entry.position = tidy(world),
        (Target::PositionKey(index), _) => {
            if let Some(track) = &mut entry.motion.position {
                set_key(track, index, tidy(world));
            }
        }
        (Target::Rotation, _) => {
            let angle = (world - frame.origin).to_angle();
            // The closest equivalent angle, so that turning on from 180° doesn't jump.
            let turn = (angle - entry.rotation + PI).rem_euclid(TAU) - PI;
            entry.rotation = (entry.rotation + turn).to_degrees().round().to_radians();
        }
        (Target::Corner, SpawnArea::Rect { half_extents, .. }) => {
            *half_extents = tidy(local).abs();
        }
        (Target::RadiusX, SpawnArea::Ellipse { radii, .. }) => radii.x = tidy(local).x.abs(),
        (Target::RadiusY, SpawnArea::Ellipse { radii, .. }) => radii.y = tidy(local).y.abs(),
        (Target::RingInner, SpawnArea::Ring { radius, .. }) => {
            radius.range.start = widgets::round_to_step(local.length(), pixel);
        }
        (Target::RingOuter, SpawnArea::Ring { radius, .. }) => {
            radius.range.end = widgets::round_to_step(local.length(), pixel);
        }
        (Target::LineFrom, SpawnArea::Line { from, .. }) => *from = tidy(local),
        (Target::LineTo, SpawnArea::Line { to, .. }) => *to = tidy(local),
        _ => apply_modifiers(target, world, &mut def.modifiers, frame.origin, pixel),
    }
}

/// Moves a modifier's handle.
fn apply_modifiers(
    target: Target,
    world: Vec2,
    modifiers: &mut [Modifier],
    origin: Vec2,
    pixel: f32,
) {
    let relative = round(world - origin, pixel);
    let (Target::Offset(index) | Target::OffsetKey(index, _) | Target::Bounds(index)) = target
    else {
        return;
    };
    match (target, modifiers.get_mut(index)) {
        (
            Target::Offset(_),
            Some(Modifier::PointAttractor { offset, .. } | Modifier::Vortex { offset, .. }),
        ) => *offset = relative.into(),
        (
            Target::OffsetKey(_, key),
            Some(
                Modifier::PointAttractor { offset: Animated::Track(track), .. }
                | Modifier::Vortex { offset: Animated::Track(track), .. },
            ),
        ) => set_key(track, key, relative),
        (Target::Bounds(_), Some(Modifier::Bounds { half_extents, .. })) => {
            *half_extents = relative.abs().into();
        }
        _ => {}
    }
}

/// Rounds to what can be told apart on the screen, or finer.
fn round(value: Vec2, pixel: f32) -> Vec2 {
    let round = |value| widgets::round_to_step(value, pixel);
    Vec2::new(round(value.x), round(value.y))
}

fn set_key(track: &mut Track<Vec2>, index: usize, value: Vec2) {
    let mut keys = track.keys().to_vec();
    if let Some(key) = keys.get_mut(index) {
        key.value = value;
        track.set_keys(keys);
    }
}

/// Whether handles are shown, which is up to the user.
fn hidden_id() -> egui::Id {
    egui::Id::new("handles hidden")
}

pub fn toggle_button(ui: &mut egui::Ui) {
    let hidden = ui.data(|data| data.get_temp::<bool>(hidden_id())).unwrap_or(false);
    if ui.add(egui::Button::new("Show handles").selected(!hidden)).clicked() {
        ui.data_mut(|data| data.insert_temp(hidden_id(), !hidden));
    }
}

/// Shows the handles of what's selected. `area` is the part of the window that shows the scene.
pub fn show(
    ctx: &egui::Context,
    scene: &str,
    selection: Selection,
    documents: &mut Documents,
    ages: &Ages,
    camera: (&Camera, &GlobalTransform),
    area: egui::Rect,
) {
    if ctx.data(|data| data.get_temp::<bool>(hidden_id())).unwrap_or(false) {
        return;
    }
    let Some(mut file) = documents.scene(scene).cloned() else {
        return;
    };
    // From the projection rather than from two points of the screen, which would be less exact
    // far from the origin.
    let pixel = match camera.0.logical_viewport_size() {
        Some(size) if size.x >= 1.0 => 2.0 / (camera.0.clip_from_view().x_axis.x * size.x),
        _ => return,
    };
    match selection {
        Selection::Scene => {
            let mut overlay = Overlay::default();
            modifiers_overlay(&mut overlay, &file.modifiers, Vec2::ZERO);
            let id = egui::Id::new((scene, "scene"));
            if let Some((target, world)) = show_overlay(ctx, id, &overlay, camera, area) {
                apply_modifiers(target, world, &mut file.modifiers, Vec2::ZERO, pixel);
                documents.edit_scene(scene, file);
            }
        }
        Selection::Emitter(index) => {
            let age = ages.emitters.get(index).copied().unwrap_or(0.0);
            show_emitter(ctx, scene, index, file, documents, age, camera, pixel, area);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn show_emitter(
    ctx: &egui::Context,
    scene: &str,
    index: usize,
    mut file: EffectFile,
    documents: &mut Documents,
    age: f32,
    camera: (&Camera, &GlobalTransform),
    pixel: f32,
    area: egui::Rect,
) {
    let Some(entry) = file.emitters.get_mut(index) else {
        return;
    };
    let path = match &entry.source {
        EmitterSource::File(file) => emitter_path(scene, file),
        EmitterSource::Inline(_) => None,
    };
    let def = match &entry.source {
        EmitterSource::Inline(def) => Some((**def).clone()),
        EmitterSource::File(_) => path.as_ref().and_then(|path| documents.emitter(path)).cloned(),
    };
    let Some(mut def) = def else {
        return;
    };
    let overlay = emitter_overlay(entry, &def, age, pixel);
    let id = egui::Id::new((scene, index));
    let Some((target, world)) = show_overlay(ctx, id, &overlay, camera, area) else {
        return;
    };
    apply_emitter(target, world, entry, &mut def, age, pixel);
    match &mut entry.source {
        EmitterSource::Inline(inline) => **inline = def,
        EmitterSource::File(_) => {
            if let Some(path) = path {
                documents.edit_emitter(&path, def);
            }
        }
    }
    documents.edit_scene(scene, file);
}

/// Returns what handle is being dragged, and where to.
fn show_overlay(
    ctx: &egui::Context,
    id: egui::Id,
    overlay: &Overlay,
    (camera, camera_transform): (&Camera, &GlobalTransform),
    area: egui::Rect,
) -> Option<(Target, Vec2)> {
    let to_screen = |world: Vec2| {
        let position = camera.world_to_viewport(camera_transform, world.extend(0.0)).ok()?;
        Some(egui::pos2(position.x, position.y))
    };
    let to_world = |screen: egui::Pos2| {
        camera.viewport_to_world_2d(camera_transform, Vec2::new(screen.x, screen.y)).ok()
    };

    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("handle lines"));
    let painter = ctx.layer_painter(layer).with_clip_rect(area);
    for outline in &overlay.outlines {
        let points: Vec<_> = outline.points.iter().filter_map(|point| to_screen(*point)).collect();
        // Dark underneath, to show on any color.
        for (width, color) in [(3.0, Color32::from_black_alpha(120)), (1.0, OUTLINE)] {
            let stroke = egui::Stroke::new(width, color);
            painter.add(if outline.closed {
                egui::Shape::closed_line(points.clone(), stroke)
            } else {
                egui::Shape::line(points.clone(), stroke)
            });
        }
    }

    let mut dragged = None;
    for handle in &overlay.handles {
        let id = id.with(handle.target);
        let grab_id = id.with("grab");
        let Some(center) = to_screen(handle.at) else {
            continue;
        };
        // Handles that are being dragged stay, wherever they end up.
        let held = ctx.data(|data| data.get_temp::<Vec2>(grab_id)).is_some();
        if !held && !area.contains(center) {
            continue;
        }
        let size = egui::Vec2::splat(HANDLE_SIZE);
        let response = egui::Area::new(id)
            .order(egui::Order::Middle)
            .fixed_pos(center - size / 2.0)
            .constrain(false)
            .fade_in(false)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(size, Sense::drag());
                draw_handle(ui, rect.center(), handle.target, &response);
                response
            })
            .inner;
        if response.drag_started()
            && let Some(start) = ctx.input(|input| input.pointer.press_origin()).and_then(to_world)
        {
            // Where in the handle it was taken hold of.
            ctx.data_mut(|data| data.insert_temp(grab_id, handle.at - start));
        }
        if response.dragged() {
            let grab = ctx.data(|data| data.get_temp::<Vec2>(grab_id)).unwrap_or_default();
            if response.drag_delta() != egui::Vec2::ZERO
                && let Some(world) = response.interact_pointer_pos().and_then(to_world)
            {
                dragged = Some((handle.target, world + grab));
            }
        } else if held {
            ctx.data_mut(|data| data.remove::<Vec2>(grab_id));
        }
        if response.hovered() && !response.dragged() {
            response.on_hover_text(&handle.label);
        }
    }
    dragged
}

fn draw_handle(ui: &egui::Ui, center: egui::Pos2, target: Target, response: &egui::Response) {
    let active = response.hovered() || response.dragged();
    let fill = if active { Color32::WHITE } else { theme::KEYED };
    let stroke = egui::Stroke::new(2.0, Color32::from_black_alpha(200));
    let painter = ui.painter();
    match target {
        Target::PositionKey(_) | Target::OffsetKey(..) => {
            let points = [(0.0, -7.0), (7.0, 0.0), (0.0, 7.0), (-7.0, 0.0)];
            let points = points.map(|(x, y)| center + egui::vec2(x, y)).to_vec();
            painter.add(egui::Shape::convex_polygon(points, fill, stroke));
        }
        Target::Position | Target::Offset(_) => {
            painter.circle(center, 7.0, fill, stroke);
            painter.circle_filled(center, 2.0, Color32::BLACK);
        }
        Target::Rotation => {
            painter.circle(center, 6.0, theme::WARNING, stroke);
        }
        _ => {
            let rect = egui::Rect::from_center_size(center, egui::Vec2::splat(11.0));
            painter.rect(rect, 1.0, fill, stroke, egui::StrokeKind::Outside);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_particle::{EmitterMotion, Fill, Modifiers};

    fn entry() -> EmitterEntry {
        EmitterEntry {
            position: Vec2::new(100.0, -50.0),
            rotation: 0.5,
            motion: default(),
            source: EmitterSource::Inline(default()),
        }
    }

    fn areas() -> [SpawnArea; 5] {
        [
            SpawnArea::Point,
            SpawnArea::Rect { half_extents: Vec2::new(30.0, 20.0), fill: Fill::Area },
            SpawnArea::Ellipse { radii: Vec2::new(30.0, 20.0), fill: Fill::Edge },
            SpawnArea::Ring { radius: (40.0..60.0).into(), arc: (0.0..3.0).into() },
            SpawnArea::Line { from: Vec2::new(-30.0, 0.0), to: Vec2::new(50.0, 10.0) },
        ]
    }

    fn def(spawn: SpawnArea) -> EmitterDef {
        let path = Track::over_life([(0.0, Vec2::new(1.0, 2.0)), (1.0, Vec2::new(30.0, 40.0))]);
        let mut def = EmitterDef {
            modifiers: Modifiers(vec![
                Modifier::Drag(1.0.into()),
                Modifier::Vortex {
                    offset: Vec2::new(5.0, 6.0).into(),
                    strength: 1.0.into(),
                    max_accel: 1.0.into(),
                },
                Modifier::PointAttractor {
                    offset: path.into(),
                    strength: 1.0.into(),
                    max_accel: 1.0.into(),
                },
                Modifier::Bounds {
                    half_extents: Vec2::new(70.0, 80.0).into(),
                    restitution: 1.0.into(),
                },
            ]),
            ..default()
        };
        def.emitter.spawn = spawn;
        def
    }

    fn at(overlay: &Overlay, target: Target) -> Vec2 {
        overlay.handles.iter().find(|handle| handle.target == target).unwrap().at
    }

    #[test]
    fn handles_go_where_they_are_dragged() {
        for area in areas() {
            let (entry, def) = (entry(), def(area));
            let before = emitter_overlay(&entry, &def, 0.0, 1.0);
            for handle in &before.handles {
                let (mut entry, mut def) = (entry.clone(), def.clone());
                // In a direction that each of them can go, away from the emitter.
                let goal = handle.at + (handle.at - entry.position).normalize_or(Vec2::X) * 25.0;
                apply_emitter(handle.target, goal, &mut entry, &mut def, 0.0, 1.0);
                let after = emitter_overlay(&entry, &def, 0.0, 1.0);
                let moved = at(&after, handle.target);
                if handle.target != Target::Rotation {
                    // Values are rounded.
                    assert!(moved.distance(goal) < 1.5, "{handle:?} went to {moved}, not {goal}");
                }
            }
        }
    }

    #[test]
    fn handles_that_stay_change_nothing() {
        for area in areas() {
            let (mut entry, mut def) = (entry(), def(area));
            entry.rotation = 30f32.to_radians();
            let before = (entry.clone(), def.clone());
            for handle in emitter_overlay(&entry, &def, 0.0, 1.0).handles {
                apply_emitter(handle.target, handle.at, &mut entry, &mut def, 0.0, 1.0);
                assert!(
                    (entry.rotation - before.0.rotation).abs() < 1e-5,
                    "{handle:?} turned the emitter"
                );
                entry.rotation = before.0.rotation;
                assert_eq!((&entry, &def), (&before.0, &before.1), "{handle:?}");
            }
        }
    }

    #[test]
    fn handles_are_finer_the_closer_the_view() {
        let (mut entry, mut def) = (entry(), def(SpawnArea::Point));
        let goal = Vec2::new(12.345, 6.789);
        apply_emitter(Target::Position, goal, &mut entry, &mut def, 0.0, 0.1);
        assert!(entry.position.distance(Vec2::new(12.3, 6.8)) < 1e-4);
        let tip = at(&emitter_overlay(&entry, &def, 0.0, 0.1), Target::Rotation);
        assert!((tip.distance(entry.position) - ROTATION_DISTANCE * 0.1).abs() < 1e-3);
    }

    #[test]
    fn rotation_goes_on_past_half_a_turn() {
        let (mut entry, mut def) = (entry(), def(SpawnArea::Point));
        entry.rotation = 170f32.to_radians();
        let goal = entry.position + Vec2::from_angle((-170f32).to_radians()) * 40.0;
        apply_emitter(Target::Rotation, goal, &mut entry, &mut def, 0.0, 1.0);
        assert!((entry.rotation.to_degrees() - 190.0).abs() < 0.01);
    }

    #[test]
    fn keyframed_values_only_have_handles_for_their_keys() {
        let (mut entry, mut def) = (entry(), def(areas()[1].clone()));
        entry.motion = EmitterMotion {
            position: Some(Track::over_time([(0.0, Vec2::ZERO), (2.0, Vec2::new(10.0, 0.0))])),
            rotation: Some(Track::over_time([(0.0, 1.0)])),
        };
        let spawn = r#"(spawn: Rect(half_extents: (clock: Time, keys: [(time: 0.0, value: (9.0, 9.0))])))"#;
        let text = format!("#![enable(implicit_some)]\n{spawn}");
        def.tracks = ron::from_str(&text).unwrap();
        let overlay = emitter_overlay(&entry, &def, 1.0, 1.0);
        let has = |target| overlay.handles.iter().any(|handle| handle.target == target);
        assert!(has(Target::PositionKey(0)) && has(Target::PositionKey(1)));
        assert!(!has(Target::Position) && !has(Target::Rotation) && !has(Target::Corner));
        // The area is where the emitter is now, and as large as it is now.
        let corner = overlay.outlines[0].points[0];
        let expected = Vec2::new(5.0, 0.0) + Vec2::from_angle(1.0).rotate(Vec2::splat(9.0));
        assert!(corner.distance(expected) < 1e-3);

        apply_emitter(Target::PositionKey(1), Vec2::new(7.0, 8.0), &mut entry, &mut def, 1.0, 1.0);
        let track = entry.motion.position.unwrap();
        assert_eq!((track.keys()[1].time, track.keys()[1].value), (2.0, Vec2::new(7.0, 8.0)));
    }
}
