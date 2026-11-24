//! What's in a scene, and the settings of the part that's selected.

use std::collections::BTreeSet;

use bevy::{asset::AssetPath, math::Vec2};
use bevy_egui::egui::{self, Ui};
use bevy_particle::{
    Animated, AreaTracks, Blend, BlendMode, EffectFile, EmitterDef, EmitterEntry, EmitterSource,
    EmitterTracks, Fill, Gravity, GravityMethod, Modifier, Modifiers, ParticleEmitter, Phase,
    Playback, RenderFile, Rgb, SpawnArea, SpawnDirection, Spread, TargetAttractor, TextureFile,
    Track,
};

use super::{
    curves::{self, Context, Keys, Look, Value},
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

/// How long the parts of the scene on screen have been running.
#[derive(Debug, Default)]
pub struct Ages {
    pub scene: Option<f32>,
    pub emitters: Vec<f32>,
}

/// What surrounds the modifiers and emitter settings being shown.
#[derive(Clone, Copy)]
struct Owner {
    age: Option<f32>,
    /// Whether any of its particles never die.
    immortal: bool,
}

impl Owner {
    /// For keyframes of the owner itself.
    fn context(self) -> Context<'static> {
        Context { time_only: true, age: self.age, life_warning: None }
    }

    /// For keyframes of modifier parameters.
    fn modifier_context(self, phase: Phase) -> Context<'static> {
        let warning = match phase {
            Phase::Appearance => "Doesn't affect immortal particles, which some of these are.",
            _ => "Stays at its start for immortal particles, which some of these are.",
        };
        Context {
            time_only: false,
            age: self.age,
            life_warning: self.immortal.then_some(warning),
        }
    }
}

pub fn show(
    ui: &mut Ui,
    scene: &str,
    selection: &mut Selection,
    documents: &mut Documents,
    ages: &Ages,
) {
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
            Selection::Scene => show_scene(ui, scene, &mut file, documents, ages.scene),
            Selection::Emitter(index) => {
                let entry = &mut file.emitters[index];
                let age = ages.emitters.get(index).copied();
                emitter_file = show_emitter(ui, scene, entry, documents, age);
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

fn show_scene(
    ui: &mut Ui,
    scene: &str,
    file: &mut EffectFile,
    documents: &Documents,
    age: Option<f32>,
) {
    let immortal = file.emitters.iter().any(|entry| match &entry.source {
        EmitterSource::Inline(def) => def.emitter.lifetime.is_none(),
        EmitterSource::File(path) => emitter_path(scene, path)
            .and_then(|path| documents.emitter(&path))
            .is_some_and(|def| def.emitter.lifetime.is_none()),
    });
    let owner = Owner { age, immortal };
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
    section(ui, "Mouse attractor", |ui| {
        show_attractor(ui, &mut file.attractor, owner);
    });
    section(ui, "Modifiers", |ui| {
        ui.weak("For every particle in the scene");
        show_modifiers(ui, &mut file.modifiers, owner);
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

fn show_attractor(ui: &mut Ui, attractor: &mut Option<TargetAttractor>, owner: Owner) {
    let default = TargetAttractor { strength: 2e6.into(), max_accel: 400.0.into() };
    let hint = "Pulls particles to the mouse while its button is held";
    widgets::optional(ui, "Enabled", hint, attractor, default);
    if let Some(attractor) = attractor {
        // It's applied like a force.
        let context = owner.modifier_context(Phase::Force);
        let TargetAttractor { strength, max_accel } = attractor;
        animated_number(ui, "Strength", "Negative repels", strength, &STRENGTH, context);
        let hint = "Limit to the acceleration";
        animated_number(ui, "Max. accel.", hint, max_accel, &ACCELERATION, context);
    }
}

/// Returns the emitter file to change, if the emitter has one, and what to.
fn show_emitter(
    ui: &mut Ui,
    scene: &str,
    entry: &mut EmitterEntry,
    documents: &Documents,
    age: Option<f32>,
) -> Option<(String, EmitterDef)> {
    section(ui, "Placement", |ui| {
        let context = Context { time_only: true, age, life_warning: None };
        let EmitterEntry { position, rotation, motion, .. } = entry;
        let (look, start) = (vec2_look(&DISTANCE), *position);
        keyed(ui, "Position", "", &look, &mut motion.position, context, start, |ui| {
            widgets::vec2(ui, position, &DISTANCE);
        });
        tracked_number(ui, "Rotation", "", rotation, &SIGNED_ANGLE, &mut motion.rotation, context);
    });
    match &mut entry.source {
        EmitterSource::Inline(def) => {
            show_def(ui, def, age);
            None
        }
        EmitterSource::File(file) => {
            let path = emitter_path(scene, file)?;
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
            show_def(ui, &mut def, age);
            Some((path, def))
        }
    }
}

/// Where the emitter file is that a scene calls `file`.
fn emitter_path(scene: &str, file: &str) -> Option<String> {
    Some(document_path(&AssetPath::parse(scene).resolve_embed_str(file).ok()?))
}

fn show_def(ui: &mut Ui, def: &mut EmitterDef, age: Option<f32>) {
    let EmitterDef { emitter, tracks: stored_tracks, modifiers, attractor } = def;
    let owner = Owner { age, immortal: emitter.lifetime.is_none() };
    let context = owner.context();
    let mut tracks = stored_tracks.clone().unwrap_or_default();
    section(ui, "Spawning", |ui| {
        let rate = Number::new(0.0, 100_000.0).log().at_least(0.0).suffix("/s");
        tracked_number(ui, "Rate", "", &mut emitter.rate, &rate, &mut tracks.rate, context);
        let hint = "Spawned once, at the start. Restart the scene to see a change.";
        let burst = Number::new(0.0, 1_000_000.0).log().at_least(0.0);
        widgets::integer(ui, "Burst", hint, &mut emitter.burst_count, &burst);
        let mortal = emitter.lifetime.is_some();
        let hint = "Without, particles never die";
        widgets::optional(ui, "Limited life", hint, &mut emitter.lifetime, (2.0..4.0).into());
        if let Some(lifetime) = &mut emitter.lifetime {
            let seconds = Number::new(0.0, 20.0).log().at_least(0.001).suffix(" s");
            spread(ui, "Lifetime", "", lifetime, &seconds, &mut tracks.lifetime, context);
        } else if mortal {
            // It would be an error now.
            tracks.lifetime = None;
        }
        show_area(ui, &mut emitter.spawn, &mut tracks.spawn, context);
        let hint = "Whether these particles take part in the scene's gravity";
        widgets::checkbox(ui, "Gravity", hint, &mut emitter.gravity);
    });
    section(ui, "Motion", |ui| {
        let speed = Number::new(-1000.0, 1000.0).log();
        spread(ui, "Speed", "", &mut emitter.speed, &speed, &mut tracks.speed, context);
        widgets::row(ui, "Direction", "What the angle is measured from", |ui| {
            let options = [
                (SpawnDirection::Fixed, "Fixed"),
                (SpawnDirection::Outward, "Outward"),
                (SpawnDirection::Normal, "Normal"),
            ];
            widgets::choice(ui, "direction", &mut emitter.direction, &options);
        });
        spread(ui, "Angle", "", &mut emitter.angle, &TURN, &mut tracks.angle, context);
        let hint = "How far particles are turned; shows on textures and stretched disks";
        // Not to be confused with that of the emitter.
        let (value, track) = (&mut emitter.rotation, &mut tracks.rotation);
        spread(ui, "Rotation", hint, value, &TURN, track, context);
        let spin = Number::angle(-720.0, 720.0).suffix("°/s");
        let hint = "How fast particles turn";
        spread(ui, "Spin", hint, &mut emitter.spin, &spin, &mut tracks.spin, context);
    });
    section(ui, "Appearance", |ui| {
        let size = Number::new(0.0, 100.0).log().at_least(0.0);
        spread(ui, "Radius", "", &mut emitter.radius, &size, &mut tracks.radius, context);
        let mass = Number::new(0.0, 1000.0).log();
        let hint = "Mass is the radius times this. Also changes existing particles.";
        let (value, track) = (&mut emitter.mass_per_radius, &mut tracks.mass_per_radius);
        tracked_number(ui, "Mass / radius", hint, value, &mass, track, context);
        show_color(ui, emitter, &mut tracks, context);
        spread(ui, "Opacity", "", &mut emitter.alpha, &UNIT, &mut tracks.alpha, context);
    });
    section(ui, "Mouse attractor", |ui| show_attractor(ui, attractor, owner));
    section(ui, "Modifiers", |ui| {
        ui.weak("For this emitter's particles");
        show_modifiers(ui, modifiers, owner);
    });
    store_tracks(stored_tracks, tracks);
}

/// Puts changed tracks back, if there are any.
fn store_tracks(stored: &mut Option<EmitterTracks>, tracks: EmitterTracks) {
    let none = EmitterTracks::default();
    if *stored.as_ref().unwrap_or(&none) != tracks {
        *stored = (tracks != none).then_some(tracks);
    }
}

fn number_look(number: &Number) -> Look {
    Look::Curves(&["Value"], number.clone())
}

fn vec2_look(number: &Number) -> Look {
    Look::Curves(&["x", "y"], number.clone())
}

fn range_look(number: &Number) -> Look {
    Look::Curves(&["from", "to"], number.clone())
}

/// A row with either a constant value or keyframes, and the editor of those. New keyframes have
/// the value `start`.
fn keyed<T: Value>(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    look: &Look,
    track: &mut Option<Track<T>>,
    context: Context,
    start: T,
    constant: impl FnOnce(&mut Ui),
) {
    let mut keys = track.as_ref().map(Keys::of);
    let before = keys.clone();
    let toggled = keys_row(ui, label, hint, look, &mut keys, context, constant);
    if toggled {
        *track = match track {
            Some(_) => None,
            None => Some(new_track(start, context)),
        };
    } else if keys != before
        && let (Some(keys), Some(track)) = (&keys, track)
    {
        keys.store(track);
    }
}

/// Keyframes that change nothing yet.
fn new_track<T: Value>(value: T, context: Context) -> Track<T> {
    let keys = [(0.0, value), (1.0, value)];
    if context.time_only { Track::over_time(keys) } else { Track::over_life(keys) }
}

/// What [`keyed`] shows. Returns whether keyframes should be turned on or off.
fn keys_row(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    look: &Look,
    keys: &mut Option<Keys>,
    context: Context,
    constant: impl FnOnce(&mut Ui),
) -> bool {
    ui.push_id(label, |ui| {
        let open_id = ui.id().with("open");
        let mut open = ui.data(|data| data.get_temp(open_id)).unwrap_or(false);
        let toggled = widgets::key_row(ui, label, hint, keys.is_some(), |ui| match keys {
            Some(keys) => curves::preview(ui, keys, look, &mut open),
            None => constant(ui),
        });
        if toggled {
            open = keys.is_none();
        } else if open && let Some(keys) = keys {
            let fill = egui::Color32::from_gray(32);
            egui::Frame::new().fill(fill).corner_radius(4).inner_margin(6).show(ui, |ui| {
                curves::editor(ui, keys, look, context);
            });
        }
        ui.data_mut(|data| data.insert_temp(open_id, open));
        toggled
    })
    .inner
}

/// A number of an emitter, and its track.
fn tracked_number(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut f32,
    number: &Number,
    track: &mut Option<Track<f32>>,
    context: Context,
) {
    let start = *value;
    keyed(ui, label, hint, &number_look(number), track, context, start, |ui| {
        widgets::slider(ui, value, number);
    });
}

/// A random range of an emitter, and its track.
fn spread(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Spread,
    number: &Number,
    track: &mut Option<Track<Vec2>>,
    context: Context,
) {
    let start = Vec2::new(value.range.start, value.range.end);
    keyed(ui, label, hint, &range_look(number), track, context, start, |ui| {
        widgets::spread_ends(ui, value, number);
    });
    ui.push_id((label, "shape"), |ui| {
        widgets::spread_shape(ui, value, number, track.is_none());
    });
}

/// The tracks of an area's first and second dimension.
fn area_slots(tracks: &Option<AreaTracks>) -> [Option<Track<Vec2>>; 2] {
    match tracks.clone() {
        None => [None, None],
        Some(AreaTracks::Rect { half_extents: track } | AreaTracks::Ellipse { radii: track }) => {
            [Some(track), None]
        }
        Some(AreaTracks::Ring { radius: a, arc: b } | AreaTracks::Line { from: a, to: b }) => {
            [a, b]
        }
    }
}

/// The inverse of [`area_slots`], for an area of this kind.
fn area_tracks(area: &SpawnArea, [a, b]: [Option<Track<Vec2>>; 2]) -> Option<AreaTracks> {
    match area {
        SpawnArea::Point => None,
        SpawnArea::Rect { .. } => a.map(|half_extents| AreaTracks::Rect { half_extents }),
        SpawnArea::Ellipse { .. } => a.map(|radii| AreaTracks::Ellipse { radii }),
        _ if a.is_none() && b.is_none() => None,
        SpawnArea::Ring { .. } => Some(AreaTracks::Ring { radius: a, arc: b }),
        SpawnArea::Line { .. } => Some(AreaTracks::Line { from: a, to: b }),
    }
}

fn show_area(
    ui: &mut Ui,
    area: &mut SpawnArea,
    tracks: &mut Option<AreaTracks>,
    context: Context,
) {
    let kind = std::mem::discriminant(area);
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
    if std::mem::discriminant(area) != kind {
        // They're for the other kind of area.
        *tracks = None;
    }

    let mut slots = area_slots(tracks);
    let before = slots.clone();
    let [a, b] = &mut slots;
    let fill_row = |ui: &mut Ui, fill: &mut Fill| {
        widgets::row(ui, "Fill", "", |ui| {
            widgets::choice(ui, "fill", fill, &[(Fill::Area, "Area"), (Fill::Edge, "Edge")]);
        });
    };
    match area {
        SpawnArea::Point => {}
        SpawnArea::Rect { half_extents, fill } => {
            let hint = "Half the width and height";
            tracked_vec2(ui, "Half size", hint, half_extents, &SIZE, a, context);
            fill_row(ui, fill);
        }
        SpawnArea::Ellipse { radii, fill } => {
            tracked_vec2(ui, "Radii", "", radii, &SIZE, a, context);
            fill_row(ui, fill);
        }
        SpawnArea::Ring { radius, arc } => {
            // "Radius" is taken by the particles' own.
            spread(ui, "Ring radius", "", radius, &SIZE, a, context);
            spread(ui, "Arc", "Which part of the ring", arc, &TURN, b, context);
        }
        SpawnArea::Line { from, to } => {
            tracked_vec2(ui, "From", "", from, &DISTANCE, a, context);
            tracked_vec2(ui, "To", "", to, &DISTANCE, b, context);
        }
    }
    if slots != before {
        *tracks = area_tracks(area, slots);
    }
}

fn tracked_vec2(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Vec2,
    number: &Number,
    track: &mut Option<Track<Vec2>>,
    context: Context,
) {
    let start = *value;
    keyed(ui, label, hint, &vec2_look(number), track, context, start, |ui| {
        widgets::vec2(ui, value, number);
    });
}

fn show_color(
    ui: &mut Ui,
    emitter: &mut ParticleEmitter,
    tracks: &mut EmitterTracks,
    context: Context,
) {
    let ParticleEmitter { red, green, blue, .. } = emitter;
    let EmitterTracks { red: red_track, green: green_track, blue: blue_track, .. } = tracks;
    let hint = "Each particle's red, green and blue are between those of two colors";
    let mut keys = match (&red_track, &green_track, &blue_track) {
        (Some(red), Some(green), Some(blue)) => Keys::of_channels([red, green, blue]),
        _ => None,
    };
    let separate = keys.is_none()
        && (red_track.is_some() || green_track.is_some() || blue_track.is_some());

    if separate {
        widgets::row(ui, "Color", hint, |ui| {
            ui.weak("Its channels have keyframes of their own");
        });
    } else {
        let before = keys.clone();
        let label = if keys.is_some() { "Color" } else { "Color from" };
        let toggled = keys_row(ui, label, hint, &Look::Colors, &mut keys, context, |ui| {
            let channels = [&mut red.range.start, &mut green.range.start, &mut blue.range.start];
            widgets::srgb(ui, channels);
        });
        if toggled {
            let turn_on = keys.is_none();
            for (track, spread) in
                [(&mut *red_track, &*red), (&mut *green_track, &*green), (&mut *blue_track, &*blue)]
            {
                let start = Vec2::new(spread.range.start, spread.range.end);
                *track = turn_on.then(|| new_track(start, context));
            }
        } else if keys != before
            && let (Some(keys), Some(red), Some(green), Some(blue)) =
                (&keys, &mut *red_track, &mut *green_track, &mut *blue_track)
        {
            keys.store_channels([red, green, blue]);
        }
        if keys.is_none() {
            widgets::row(ui, "Color to", hint, |ui| {
                let channels = [&mut red.range.end, &mut green.range.end, &mut blue.range.end];
                widgets::srgb(ui, channels);
            });
        }
    }
    egui::CollapsingHeader::new("Color channels").default_open(separate).show(ui, |ui| {
        let channel = Number::new(0.0, 2.0).at_least(0.0);
        spread(ui, "Red", "", red, &channel, red_track, context);
        spread(ui, "Green", "", green, &channel, green_track, context);
        spread(ui, "Blue", "", blue, &channel, blue_track, context);
    });
}

/// A modifier parameter: a constant or keyframes.
fn animated<T: Value>(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Animated<T>,
    look: &Look,
    context: Context,
    add: impl FnOnce(&mut Ui, &mut T),
) {
    let (mut constant, mut track) = match value {
        Animated::Constant(constant) => (*constant, None),
        Animated::Track(track) => {
            // What's left when its keyframes are removed.
            let first = track.keys().first().map(|key| key.value);
            (first.unwrap_or(T::store(Default::default())), Some(track.clone()))
        }
    };
    let start = constant;
    keyed(ui, label, hint, look, &mut track, context, start, |ui| add(ui, &mut constant));
    *value = match track {
        Some(track) => Animated::Track(track),
        None => Animated::Constant(constant),
    };
}

fn animated_number(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Animated<f32>,
    number: &Number,
    context: Context,
) {
    animated(ui, label, hint, value, &number_look(number), context, |ui, value| {
        widgets::slider(ui, value, number);
    });
}

fn animated_vec2(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    value: &mut Animated<Vec2>,
    number: &Number,
    context: Context,
) {
    animated(ui, label, hint, value, &vec2_look(number), context, |ui, value| {
        widgets::vec2(ui, value, number);
    });
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

fn show_modifiers(ui: &mut Ui, modifiers: &mut Modifiers, owner: Owner) {
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
                    show_modifier(ui, &mut modifiers[index], owner.modifier_context(phase));
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

fn show_modifier(ui: &mut Ui, modifier: &mut Modifier, context: Context) {
    match modifier {
        Modifier::Acceleration(acceleration) => {
            let number = Number::new(-2000.0, 2000.0).log();
            animated_vec2(ui, "Acceleration", "", acceleration, &number, context);
        }
        Modifier::PointAttractor { offset, strength, max_accel }
        | Modifier::Vortex { offset, strength, max_accel } => {
            let hint = "Relative to the scene or emitter";
            animated_vec2(ui, "Offset", hint, offset, &DISTANCE, context);
            let hint = "Negative goes the other way";
            animated_number(ui, "Strength", hint, strength, &STRENGTH, context);
            let hint = "Limit to the acceleration";
            animated_number(ui, "Max. accel.", hint, max_accel, &ACCELERATION, context);
        }
        Modifier::Drag(drag) => {
            let hint = "Fraction of the speed lost per second, roughly";
            let number = Number::new(0.0, 10.0).log();
            animated_number(ui, "Drag", hint, drag, &number, context);
        }
        Modifier::Bounds { half_extents, restitution } => {
            let hint = "Half the width and height";
            animated_vec2(ui, "Half size", hint, half_extents, &SIZE, context);
            let hint = "Fraction of the speed kept when bouncing";
            let number = Number::new(0.0, 1.0).at_least(0.0);
            animated_number(ui, "Bounciness", hint, restitution, &number, context);
        }
        Modifier::Color(blend, color) => {
            show_blend(ui, blend);
            animated(ui, "Color", "", color, &Look::Color, context, widgets::rgb);
        }
        Modifier::Alpha(blend, value) => {
            show_blend(ui, blend);
            let number = Number::new(0.0, 1.0).at_least(0.0);
            animated_number(ui, "Opacity", "", value, &number, context);
        }
        Modifier::Size(blend, value) | Modifier::Mass(blend, value) => {
            show_blend(ui, blend);
            animated_number(ui, "Value", "", value, &FACTOR, context);
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
                        show(ui, &scene, &mut selection, &mut documents, &Ages::default());
                    });
                    output.textures_delta.clear();
                }
            }
            assert!(!documents.can_undo(&scene), "{scene}");
        }
        assert!(documents.dirty_files(None).is_empty());
    }

    #[test]
    fn tracks_are_only_stored_if_there_are_any() {
        let mut stored = None;
        store_tracks(&mut stored, EmitterTracks::default());
        assert!(stored.is_none());
        let tracks =
            EmitterTracks { rate: Some(Track::over_time([(0.0, 1.0)])), ..Default::default() };
        store_tracks(&mut stored, tracks.clone());
        assert_eq!(stored, Some(tracks));
        store_tracks(&mut stored, EmitterTracks::default());
        assert!(stored.is_none());
    }

    #[test]
    fn area_tracks_fit_their_area() {
        let track = || Some(Track::over_time([(0.0, Vec2::ONE)]));
        let areas = [
            SpawnArea::Rect { half_extents: Vec2::ONE, fill: Fill::Area },
            SpawnArea::Ellipse { radii: Vec2::ONE, fill: Fill::Area },
            SpawnArea::Ring { radius: (0.0..1.0).into(), arc: (0.0..1.0).into() },
            SpawnArea::Line { from: Vec2::ZERO, to: Vec2::ONE },
        ];
        for spawn in areas {
            let slots = [track(), None];
            let tracks = area_tracks(&spawn, slots.clone());
            assert_eq!(area_slots(&tracks), slots);
            let emitter = ParticleEmitter { spawn: spawn.clone(), ..Default::default() };
            let tracks = EmitterTracks { spawn: tracks, ..Default::default() };
            assert!(tracks.check(&emitter).is_ok());
            assert!(area_tracks(&spawn, [None, None]).is_none());
        }
    }

    #[test]
    fn every_modifier_can_be_added() {
        let names: BTreeSet<_> = new_modifiers().iter().map(modifier_name).collect();
        assert_eq!(names.len(), new_modifiers().len());
    }
}
