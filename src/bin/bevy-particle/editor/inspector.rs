//! What's in a scene, and the settings of the part that's selected.

use std::collections::BTreeSet;

use bevy::{asset::AssetPath, math::Vec2};
use bevy_egui::egui::{self, Ui};
use bevy_particle::{
    Animated, Blend, BlendMode, EffectFile, EmitterDef, EmitterEntry, EmitterSource, Fill, Gravity,
    GravityMethod, Keyable, Modifier, Modifiers, ParticleEmitter, Phase, Playback, RenderFile, Rgb,
    SpawnArea, SpawnDirection, Spread, TargetAttractor, TextureFile, Track,
};

use super::{
    documents::{document_path, texture_path, Documents},
    theme,
    widgets::{self, Number},
};

const DISTANCE: Number = Number::new(-1000.0, 1000.0);
const SIZE: Number = Number::new(0.0, 1000.0).log().at_least(0.0);
const SIGNED_ANGLE: Number = Number::angle(-180.0, 180.0);
const TURN: Number = Number::angle(-360.0, 360.0);
const STRENGTH: Number = Number::new(-1e7, 1e7).log();
const ACCELERATION: Number = Number::new(0.0, 5000.0).log().at_least(0.0);
const FACTOR: Number = Number::new(0.0, 10.0).log().at_least(0.0);
const UNIT: Number = Number::new(0.0, 1.0).at_least(0.0).at_most(1.0);

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Selection {
    #[default]
    Scene,
    Emitter(usize),
}

pub fn show(ui: &mut Ui, scene: &str, selection: &mut Selection, documents: &mut Documents) {
    let Some(mut file) = documents.scene(scene).cloned() else {
        return;
    };
    if let Selection::Emitter(index) = *selection
        && index >= file.emitters.len()
    {
        *selection = Selection::Scene;
    }
    show_tree(ui, &file, selection);
    ui.separator();

    let mut emitter_file = None;
    egui::ScrollArea::vertical().id_salt("inspector").auto_shrink(false).show(ui, |ui| {
        // So that the scroll bar doesn't cover anything.
        ui.set_max_width(ui.available_width() - 10.0);
        // What widgets remember is about this file and this part of it.
        ui.push_id((scene, format!("{selection:?}")), |ui| match *selection {
            Selection::Scene => show_scene(ui, scene, &mut file, documents),
            Selection::Emitter(index) => {
                let entry = &mut file.emitters[index];
                emitter_file = show_emitter(ui, scene, entry, documents);
            }
        });
    });

    documents.edit_scene(scene, file);
    if let Some((path, def)) = emitter_file {
        documents.edit_emitter(&path, def);
    }
    // A drag, or typing into a field, is a single step to undo.
    let busy = ui.input(|input| input.pointer.any_down() || input.pointer.any_released())
        || ui.ctx().egui_wants_keyboard_input();
    if !busy {
        documents.end_group();
    }
}

fn show_tree(ui: &mut Ui, file: &EffectFile, selection: &mut Selection) {
    let rows = file.emitters.len() as f32 + 1.0;
    egui::ScrollArea::vertical()
        .id_salt("tree")
        .max_height((rows * 24.0).min(140.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.selectable_value(selection, Selection::Scene, "Scene");
            for (index, entry) in file.emitters.iter().enumerate() {
                let name = match &entry.source {
                    EmitterSource::File(path) => {
                        let name = path.rsplit('/').next().unwrap_or(path);
                        format!("Emitter {} ({name})", index + 1)
                    }
                    EmitterSource::Inline(_) => format!("Emitter {}", index + 1),
                };
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.selectable_value(selection, Selection::Emitter(index), name);
                });
            }
        });
}

fn section(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(egui::RichText::new(name).strong())
        .id_salt(name)
        .default_open(true)
        .show_unindented(ui, add);
}

fn show_scene(ui: &mut Ui, scene: &str, file: &mut EffectFile, documents: &Documents) {
    section(ui, "Space", |ui| {
        widgets::integer_on_release(
            ui,
            "Capacity",
            "Most particles at a time. Changing it restarts the scene.",
            &mut file.space.capacity,
            &Number::new(1.0, 2_000_000.0).log().at_least(1.0),
        );
    });
    section(ui, "Rendering", |ui| show_render(ui, scene, &mut file.render, documents));
    section(ui, "Gravity", |ui| show_gravity(ui, &mut file.gravity));
    section(ui, "Mouse attractor", |ui| show_attractor(ui, &mut file.attractor));
    section(ui, "Modifiers", |ui| {
        ui.weak("For every particle in the scene");
        show_modifiers(ui, &mut file.modifiers);
    });
}

fn show_render(ui: &mut Ui, scene: &str, render: &mut RenderFile, documents: &Documents) {
    widgets::row(ui, "Blend", "How particles combine with what's behind them", |ui| {
        let options = [(BlendMode::Alpha, "Alpha"), (BlendMode::Additive, "Additive")];
        widgets::choice(ui, "blend", &mut render.blend, &options);
    });
    let hint = "How much of a disk fades out. Textures ignore it.";
    widgets::number(ui, "Softness", hint, &mut render.softness, &UNIT);
    let hint = "Stretches particles along their motion, by how far they get in this time";
    let seconds = Number::new(0.0, 1.0).log().at_least(0.0).suffix(" s");
    widgets::number(ui, "Stretch", hint, &mut render.stretch, &seconds);

    let textures = textures(documents);
    let current =
        render.texture.as_ref().map(|texture| texture_path(scene, &texture.path).unwrap_or_default());
    let mut picked = current.clone();
    widgets::row(ui, "Texture", "", |ui| {
        let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
        egui::ComboBox::from_id_salt("texture")
            .selected_text(current.as_deref().map_or("None".into(), name))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut picked, None, "None");
                for path in &textures {
                    ui.selectable_value(&mut picked, Some(path.clone()), name(path));
                }
            });
    });
    if picked != current {
        render.texture = picked.map(|path| {
            let path = format!("/{path}");
            match render.texture.take() {
                Some(texture) => TextureFile { path, ..texture },
                None => {
                    TextureFile { path, columns: 1, rows: 1, playback: Playback::OverLife }
                }
            }
        });
    }
    if let Some(texture) = &mut render.texture {
        let frames = Number::new(1.0, 16.0).at_least(1.0);
        let hint = "Of animation frames in the image";
        widgets::integer(ui, "Columns", hint, &mut texture.columns, &frames);
        widgets::integer(ui, "Rows", hint, &mut texture.rows, &frames);
        widgets::row(ui, "Playback", "Which frame each particle shows", |ui| {
            let options = [
                ("Over life", Playback::OverLife),
                ("Looping", Playback::Fps(12.0)),
                ("Random frame", Playback::Random),
            ];
            widgets::variant(ui, "playback", &mut texture.playback, &options);
        });
        if let Playback::Fps(fps) = &mut texture.playback {
            let number = Number::new(0.0, 60.0);
            widgets::number(ui, "Frames / s", "", fps, &number);
        }
    }
}

/// Images to choose from, as paths from the asset folder: those in the texture folder, where it
/// can be listed, and those that scenes use.
fn textures(documents: &Documents) -> BTreeSet<String> {
    let mut textures = documents.textures();
    textures.extend(super::platform::list_textures());
    textures
}

fn show_gravity(ui: &mut Ui, gravity: &mut Option<Gravity>) {
    let default = Gravity {
        strength: 1.0,
        softening: 5.0,
        method: GravityMethod::BarnesHut,
        theta: 0.5,
    };
    let hint = "Particles attract each other, if their emitter has gravity too";
    widgets::optional(ui, "Enabled", hint, gravity, default);
    let Some(gravity) = gravity else {
        return;
    };
    widgets::number(ui, "Strength", "", &mut gravity.strength, &Number::new(-100.0, 100.0).log());
    let hint = "Weakens close encounters, up to about this distance";
    let number = Number::new(0.0, 100.0).log().at_least(0.0);
    widgets::number(ui, "Softening", hint, &mut gravity.softening, &number);
    widgets::row(ui, "Method", "", |ui| {
        let options =
            [(GravityMethod::BarnesHut, "Barnes-Hut"), (GravityMethod::BruteForce, "Brute force")];
        widgets::choice(ui, "method", &mut gravity.method, &options);
    });
    if gravity.method == GravityMethod::BarnesHut {
        let hint = "Smaller is more accurate and slower";
        let number = Number::new(0.1, 1.5).at_least(0.0);
        widgets::number(ui, "Theta", hint, &mut gravity.theta, &number);
    }
}

fn show_attractor(ui: &mut Ui, attractor: &mut Option<TargetAttractor>) {
    let default = TargetAttractor { strength: 2e6.into(), max_accel: 400.0.into() };
    let hint = "Pulls particles to the mouse while its button is held";
    widgets::optional(ui, "Enabled", hint, attractor, default);
    if let Some(attractor) = attractor {
        animated_number(ui, "Strength", "Negative repels", &mut attractor.strength, &STRENGTH);
        let hint = "Limit to the acceleration";
        animated_number(ui, "Max. accel.", hint, &mut attractor.max_accel, &ACCELERATION);
    }
}

/// Returns the emitter file to change, if the emitter has one, and what to.
fn show_emitter(
    ui: &mut Ui,
    scene: &str,
    entry: &mut EmitterEntry,
    documents: &Documents,
) -> Option<(String, EmitterDef)> {
    section(ui, "Placement", |ui| {
        let keyed = entry.motion.position.is_some();
        tracked_row(ui, "Position", "", keyed, |ui| {
            widgets::vec2(ui, &mut entry.position, &DISTANCE);
        });
        let keyed = entry.motion.rotation.is_some();
        tracked_row(ui, "Rotation", "", keyed, |ui| {
            widgets::slider(ui, &mut entry.rotation, &SIGNED_ANGLE);
        });
    });
    match &mut entry.source {
        EmitterSource::Inline(def) => {
            show_def(ui, def);
            None
        }
        EmitterSource::File(file) => {
            let path = AssetPath::parse(scene).resolve_embed_str(file).ok()?;
            let path = document_path(&path);
            let Some(mut def) = documents.emitter(&path).cloned() else {
                ui.colored_label(theme::WARNING, format!("{path} isn't loaded."));
                return None;
            };
            let mut others = documents.scenes_using(&path);
            others.retain(|other| other != scene);
            if !others.is_empty() {
                let text = format!("The rest is shared with {}.", others.join(", "));
                ui.colored_label(theme::WARNING, text);
            }
            show_def(ui, &mut def);
            Some((path, def))
        }
    }
}

fn show_def(ui: &mut Ui, def: &mut EmitterDef) {
    let EmitterDef { emitter, tracks, modifiers, attractor } = def;
    let none = Default::default();
    let keyed = tracks.as_ref().unwrap_or(&none).clone();
    section(ui, "Spawning", |ui| {
        let rate = Number::new(0.0, 100_000.0).log().at_least(0.0).suffix("/s");
        tracked_row(ui, "Rate", "", keyed.rate.is_some(), |ui| {
            widgets::slider(ui, &mut emitter.rate, &rate);
        });
        let hint = "Spawned once, at the start. Restart the scene to see a change.";
        let burst = Number::new(0.0, 1_000_000.0).log().at_least(0.0);
        widgets::integer(ui, "Burst", hint, &mut emitter.burst_count, &burst);
        let mortal = emitter.lifetime.is_some();
        let hint = "Without, particles never die";
        widgets::optional(ui, "Limited life", hint, &mut emitter.lifetime, (2.0..4.0).into());
        if let Some(lifetime) = &mut emitter.lifetime {
            let seconds = Number::new(0.0, 20.0).log().at_least(0.001).suffix(" s");
            spread(ui, "Lifetime", "", lifetime, &seconds, keyed.lifetime.is_some());
        } else if mortal && let Some(tracks) = tracks {
            // It would be an error now.
            tracks.lifetime = None;
        }
        let kind = std::mem::discriminant(&emitter.spawn);
        show_area(ui, &mut emitter.spawn, keyed.spawn.is_some());
        if std::mem::discriminant(&emitter.spawn) != kind
            && let Some(tracks) = tracks
        {
            // They're for the other kind of area.
            tracks.spawn = None;
        }
        let hint = "Whether these particles take part in the scene's gravity";
        widgets::checkbox(ui, "Gravity", hint, &mut emitter.gravity);
    });
    section(ui, "Motion", |ui| {
        let speed = Number::new(-1000.0, 1000.0).log();
        spread(ui, "Speed", "", &mut emitter.speed, &speed, keyed.speed.is_some());
        widgets::row(ui, "Direction", "What the angle is measured from", |ui| {
            let options = [
                (SpawnDirection::Fixed, "Fixed"),
                (SpawnDirection::Outward, "Outward"),
                (SpawnDirection::Normal, "Normal"),
            ];
            widgets::choice(ui, "direction", &mut emitter.direction, &options);
        });
        spread(ui, "Angle", "", &mut emitter.angle, &TURN, keyed.angle.is_some());
        let hint = "How far particles are turned; shows on textures and stretched disks";
        spread(ui, "Rotation", hint, &mut emitter.rotation, &TURN, keyed.rotation.is_some());
        let spin = Number::angle(-720.0, 720.0).suffix("°/s");
        let hint = "How fast particles turn";
        spread(ui, "Spin", hint, &mut emitter.spin, &spin, keyed.spin.is_some());
    });
    section(ui, "Appearance", |ui| {
        let size = Number::new(0.0, 100.0).log().at_least(0.0);
        spread(ui, "Radius", "", &mut emitter.radius, &size, keyed.radius.is_some());
        let mass = Number::new(0.0, 1000.0).log();
        let hint = "Mass is the radius times this";
        tracked_row(ui, "Mass / radius", hint, keyed.mass_per_radius.is_some(), |ui| {
            widgets::slider(ui, &mut emitter.mass_per_radius, &mass);
        });
        show_color(ui, emitter, [&keyed.red, &keyed.green, &keyed.blue].map(Option::is_some));
        spread(ui, "Opacity", "", &mut emitter.alpha, &UNIT, keyed.alpha.is_some());
    });
    section(ui, "Mouse attractor", |ui| show_attractor(ui, attractor));
    section(ui, "Modifiers", |ui| {
        ui.weak("For this emitter's particles");
        show_modifiers(ui, modifiers);
    });
}

const KEYED_HINT: &str = "Keyframes replace this value while they're there";

/// A row whose value keyframes may replace.
fn tracked_row(ui: &mut Ui, label: &str, hint: &str, keyed: bool, add: impl FnOnce(&mut Ui)) {
    widgets::row(ui, label, hint, add);
    show_keyed(ui, keyed);
}

fn show_keyed(ui: &mut Ui, keyed: bool) {
    if keyed {
        widgets::row(ui, "", "", |ui| ui.weak("Keyframed").on_hover_text(KEYED_HINT));
    }
}

fn spread(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Spread,
    number: &Number,
    keyed: bool,
) {
    widgets::spread(ui, label, hint, value, number);
    show_keyed(ui, keyed);
}

fn show_area(ui: &mut Ui, area: &mut SpawnArea, keyed: bool) {
    widgets::row(ui, "Area", "Where particles spawn", |ui| {
        let options = [
            ("Point", SpawnArea::Point),
            ("Rectangle", SpawnArea::Rect { half_extents: Vec2::splat(50.0), fill: Fill::Area }),
            ("Ellipse", SpawnArea::Ellipse { radii: Vec2::splat(50.0), fill: Fill::Area }),
            (
                "Ring",
                SpawnArea::Ring {
                    radius: (40.0..50.0).into(),
                    arc: (0.0..std::f32::consts::TAU).into(),
                },
            ),
            ("Line", SpawnArea::Line { from: Vec2::new(-50.0, 0.0), to: Vec2::new(50.0, 0.0) }),
        ];
        widgets::variant(ui, "area", area, &options);
    });
    let fill_row = |ui: &mut Ui, fill: &mut Fill| {
        widgets::row(ui, "Fill", "", |ui| {
            widgets::choice(ui, "fill", fill, &[(Fill::Area, "Area"), (Fill::Edge, "Edge")]);
        });
    };
    match area {
        SpawnArea::Point => {}
        SpawnArea::Rect { half_extents, fill } => {
            let hint = "Half the width and height";
            widgets::row(ui, "Half size", hint, |ui| widgets::vec2(ui, half_extents, &SIZE));
            fill_row(ui, fill);
        }
        SpawnArea::Ellipse { radii, fill } => {
            widgets::row(ui, "Radii", "", |ui| widgets::vec2(ui, radii, &SIZE));
            fill_row(ui, fill);
        }
        SpawnArea::Ring { radius, arc } => {
            // "Radius" is taken by the particles' own.
            widgets::spread(ui, "Ring radius", "", radius, &SIZE);
            widgets::spread(ui, "Arc", "Which part of the ring", arc, &TURN);
        }
        SpawnArea::Line { from, to } => {
            widgets::row(ui, "From", "", |ui| widgets::vec2(ui, from, &DISTANCE));
            widgets::row(ui, "To", "", |ui| widgets::vec2(ui, to, &DISTANCE));
        }
    }
    show_keyed(ui, keyed);
}

fn show_color(ui: &mut Ui, emitter: &mut ParticleEmitter, keyed: [bool; 3]) {
    let ParticleEmitter { red, green, blue, .. } = emitter;
    let hint = "Each particle's red, green and blue are between those of these two";
    widgets::row(ui, "Color from", hint, |ui| {
        widgets::srgb(ui, [&mut red.range.start, &mut green.range.start, &mut blue.range.start]);
    });
    widgets::row(ui, "Color to", hint, |ui| {
        widgets::srgb(ui, [&mut red.range.end, &mut green.range.end, &mut blue.range.end]);
    });
    show_keyed(ui, keyed.contains(&true));
    egui::CollapsingHeader::new("Color channels").show(ui, |ui| {
        let channel = Number::new(0.0, 2.0).at_least(0.0);
        widgets::spread(ui, "Red", "", red, &channel);
        widgets::spread(ui, "Green", "", green, &channel);
        widgets::spread(ui, "Blue", "", blue, &channel);
    });
}

/// What a modifier parameter that has keyframes shows, until they can be edited.
fn show_track<T: Keyable>(ui: &mut Ui, track: &Track<T>) {
    let clock = format!("{:?}", track.clock).to_lowercase();
    ui.weak(format!("{} keyframes over {clock}", track.keys().len()));
}

fn animated<T: Keyable>(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Animated<T>,
    add: impl FnOnce(&mut Ui, &mut T),
) {
    widgets::row(ui, label, hint, |ui| match value {
        Animated::Constant(value) => add(ui, value),
        Animated::Track(track) => show_track(ui, track),
    });
}

fn animated_number(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Animated<f32>,
    number: &Number,
) {
    animated(ui, label, hint, value, |ui, value| {
        widgets::slider(ui, value, number);
    });
}

fn animated_vec2(ui: &mut Ui, label: &str, hint: &str, value: &mut Animated<Vec2>, number: &Number) {
    animated(ui, label, hint, value, |ui, value| widgets::vec2(ui, value, number));
}

fn modifier_name(modifier: &Modifier) -> &'static str {
    match modifier {
        Modifier::Acceleration(_) => "Acceleration",
        Modifier::PointAttractor { .. } => "Point attractor",
        Modifier::Vortex { .. } => "Vortex",
        Modifier::Drag(_) => "Drag",
        Modifier::Bounds { .. } => "Bounds",
        Modifier::Color(..) => "Color",
        Modifier::Alpha(..) => "Opacity",
        Modifier::Size(..) => "Size",
        Modifier::Mass(..) => "Mass",
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Force => "Forces",
        Phase::Constraint => "Constraints",
        Phase::Appearance => "Appearance",
    }
}

const PHASES: [Phase; 3] = [Phase::Force, Phase::Constraint, Phase::Appearance];

/// One of each, as they're added.
fn new_modifiers() -> [Modifier; 9] {
    let offset = || Vec2::ZERO.into();
    [
        Modifier::Acceleration(Vec2::new(0.0, -100.0).into()),
        Modifier::PointAttractor {
            offset: offset(),
            strength: 1e6.into(),
            max_accel: 400.0.into(),
        },
        Modifier::Vortex { offset: offset(), strength: 10_000.0.into(), max_accel: 400.0.into() },
        Modifier::Drag(0.5.into()),
        Modifier::Bounds {
            half_extents: Vec2::new(500.0, 300.0).into(),
            restitution: 0.5.into(),
        },
        Modifier::Color(Blend::Set, Rgb(1.0, 1.0, 1.0).into()),
        Modifier::Alpha(Blend::Scale, 1.0.into()),
        Modifier::Size(Blend::Scale, 1.0.into()),
        Modifier::Mass(Blend::Scale, 1.0.into()),
    ]
}

enum ListChange {
    Remove(usize),
    Swap(usize, usize),
}

fn show_modifiers(ui: &mut Ui, modifiers: &mut Modifiers) {
    let mut change = None;
    for phase in PHASES {
        // Only the order within a phase matters.
        let indices: Vec<_> =
            (0..modifiers.len()).filter(|i| modifiers[*i].phase() == phase).collect();
        if indices.is_empty() {
            continue;
        }
        ui.weak(phase_name(phase));
        for (place, &index) in indices.iter().enumerate() {
            let above = place.checked_sub(1).map(|place| indices[place]);
            let below = indices.get(place + 1).copied();
            ui.push_id(index, |ui| {
                egui::Frame::group(ui.style()).inner_margin(6).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(modifier_name(&modifiers[index]));
                        let layout = egui::Layout::right_to_left(egui::Align::Center);
                        ui.with_layout(layout, |ui| {
                            if ui.small_button("✖").on_hover_text("Remove").clicked() {
                                change = Some(ListChange::Remove(index));
                            }
                            for (other, text) in [(below, "⬇"), (above, "⬆")] {
                                let button = egui::Button::new(text).small();
                                if ui.add_enabled(other.is_some(), button).clicked() {
                                    change = other.map(|other| ListChange::Swap(index, other));
                                }
                            }
                        });
                    });
                    show_modifier(ui, &mut modifiers[index]);
                });
            });
        }
    }
    match change {
        Some(ListChange::Remove(index)) => {
            modifiers.remove(index);
        }
        Some(ListChange::Swap(a, b)) => modifiers.swap(a, b),
        None => {}
    }

    ui.menu_button("Add modifier", |ui| {
        for phase in PHASES {
            ui.weak(phase_name(phase));
            for modifier in new_modifiers() {
                if modifier.phase() == phase && ui.button(modifier_name(&modifier)).clicked() {
                    modifiers.push(modifier);
                }
            }
        }
    });
}

fn show_modifier(ui: &mut Ui, modifier: &mut Modifier) {
    match modifier {
        Modifier::Acceleration(acceleration) => {
            let number = Number::new(-2000.0, 2000.0).log();
            animated_vec2(ui, "Acceleration", "", acceleration, &number);
        }
        Modifier::PointAttractor { offset, strength, max_accel }
        | Modifier::Vortex { offset, strength, max_accel } => {
            let hint = "Relative to the scene or emitter";
            animated_vec2(ui, "Offset", hint, offset, &DISTANCE);
            let hint = "Negative goes the other way";
            animated_number(ui, "Strength", hint, strength, &STRENGTH);
            let hint = "Limit to the acceleration";
            animated_number(ui, "Max. accel.", hint, max_accel, &ACCELERATION);
        }
        Modifier::Drag(drag) => {
            let hint = "Fraction of the speed lost per second, roughly";
            animated_number(ui, "Drag", hint, drag, &Number::new(0.0, 10.0).log());
        }
        Modifier::Bounds { half_extents, restitution } => {
            animated_vec2(ui, "Half size", "Half the width and height", half_extents, &SIZE);
            let hint = "Fraction of the speed kept when bouncing";
            let number = Number::new(0.0, 1.0).at_least(0.0);
            animated_number(ui, "Bounciness", hint, restitution, &number);
        }
        Modifier::Color(blend, color) => {
            show_blend(ui, blend);
            animated(ui, "Color", "", color, widgets::rgb);
        }
        Modifier::Alpha(blend, value) => {
            show_blend(ui, blend);
            animated_number(ui, "Opacity", "", value, &Number::new(0.0, 1.0).at_least(0.0));
        }
        Modifier::Size(blend, value) | Modifier::Mass(blend, value) => {
            show_blend(ui, blend);
            animated_number(ui, "Value", "", value, &FACTOR);
        }
    }
}

fn show_blend(ui: &mut Ui, blend: &mut Blend) {
    widgets::row(ui, "Blend", "Scale keeps the variation between particles", |ui| {
        let options = [(Blend::Set, "Set"), (Blend::Scale, "Scale")];
        widgets::choice(ui, "blend", blend, &options);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows every part of every shipped scene, which must not change anything.
    #[test]
    fn looking_at_scenes_doesnt_change_them() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/effects");
        let mut documents = Documents::default();
        let mut scenes = Vec::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let text = std::fs::read_to_string(format!("{root}/{name}")).unwrap();
            let path = format!("effects/{name}");
            documents.file_loaded(&path, text).unwrap();
            if name.ends_with(".effect.ron") {
                scenes.push(path);
            }
        }
        for scene in scenes {
            let count = documents.scene(&scene).unwrap().emitters.len();
            let selections = (0..count).map(Selection::Emitter).chain([Selection::Scene]);
            let ctx = egui::Context::default();
            for mut selection in selections {
                for _ in 0..3 {
                    let input = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(360.0, 4000.0),
                        )),
                        ..Default::default()
                    };
                    let mut output = ctx.run_ui(input, |ui| {
                        show(ui, &scene, &mut selection, &mut documents);
                    });
                    output.textures_delta.clear();
                }
            }
            assert!(!documents.can_undo(&scene), "{scene}");
        }
        assert!(documents.dirty_files(None).is_empty());
    }

    #[test]
    fn every_modifier_can_be_added() {
        let names: BTreeSet<_> = new_modifiers().iter().map(modifier_name).collect();
        assert_eq!(names.len(), new_modifiers().len());
    }
}
