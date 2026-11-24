//! Particle effects stored in asset files.
//!
//! An `.effect.ron` file describes a whole [`ParticleEffect`]: a space and the emitters in it.
//! Emitters are written inline or kept in their own files, which effects can share. With file
//! watching on, saving either kind of file reloads the effects that use it.

use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    platform::collections::HashMap,
    prelude::*,
};
use serde::{Deserialize, Serialize};

use crate::{
    effect_file::EffectFile,
    emitter::ParticleEmitter,
    emitter_tracks::{EmitterMotion, EmitterTracks},
    gravity::Gravity,
    modifier::{Modifiers, TargetAttractor},
    render::ParticleRender,
    space::ParticleSpace,
    util::is_default,
};

/// Registers [`ParticleEffect`] and its loader.
pub(crate) struct EffectPlugin;

impl Plugin for EffectPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<ParticleEffect>()
            .init_asset_loader::<EffectLoader>();
    }
}

/// A space and the emitters in it, loaded from an `.effect.ron` file. See [`Self::spawn`].
#[derive(Asset, TypePath, Debug, Clone, PartialEq)]
pub struct ParticleEffect {
    pub space: ParticleSpace,
    pub render: ParticleRender,
    pub gravity: Option<Gravity>,
    /// Pulls every particle in the space; emitters can have their own too.
    pub attractor: Option<TargetAttractor>,
    /// Applied to every particle in the space.
    pub modifiers: Modifiers,
    pub emitters: Vec<EffectEmitter>,
}

/// An emitter placed in a [`ParticleEffect`].
#[derive(Debug, Clone, PartialEq)]
pub struct EffectEmitter {
    /// Relative to the space.
    pub position: Vec2,
    /// Radians, counter-clockwise.
    pub rotation: f32,
    pub motion: EmitterMotion,
    pub def: EmitterDef,
}

/// An emitter and the behaviors attached to it. Emitter files contain one of these.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmitterDef {
    #[serde(skip_serializing_if = "is_default")]
    pub emitter: ParticleEmitter,
    #[serde(skip_serializing_if = "is_default")]
    pub tracks: Option<EmitterTracks>,
    #[serde(skip_serializing_if = "is_default")]
    pub modifiers: Modifiers,
    #[serde(skip_serializing_if = "is_default")]
    pub attractor: Option<TargetAttractor>,
}

impl ParticleEffect {
    /// Spawns the space, with its emitters as children, and returns the space's entity.
    /// Despawning that entity removes the whole effect.
    pub fn spawn(&self, commands: &mut Commands) -> Entity {
        let mut space = commands.spawn((
            self.space.clone(),
            self.render.clone(),
            self.modifiers.clone(),
        ));
        if let Some(gravity) = self.gravity {
            space.insert(gravity);
        }
        if let Some(attractor) = &self.attractor {
            space.insert(attractor.clone());
        }
        let space = space.id();

        for emitter in &self.emitters {
            emitter.spawn(commands, space);
        }
        space
    }

    /// Changes what `old` spawned into this effect, leaving existing particles in place (except
    /// those of removed emitters). `emitters` are the space's child emitters, in order.
    ///
    /// Returns `false`, without doing anything, if only a respawn can make the change, which is to
    /// the capacity. A changed burst shows the next time the effect is spawned.
    pub fn update(
        &self,
        old: &Self,
        commands: &mut Commands,
        space: Entity,
        emitters: &[Entity],
    ) -> bool {
        if self.space != old.space || old.emitters.len() != emitters.len() {
            return false;
        }
        let mut entity = commands.entity(space);
        entity.insert((self.render.clone(), self.modifiers.clone()));
        set_component(&mut entity, self.gravity);
        set_component(&mut entity, self.attractor.clone());

        // Where emitters were added or removed, the ones around them keep their entities.
        let (new, old) = (&self.emitters, &old.emitters);
        let same = new.iter().zip(old).take_while(|(new, old)| new == old).count();
        let most = new.len().min(old.len()) - same;
        let same_at_end = new.iter().rev().zip(old.iter().rev());
        let same_at_end = same_at_end.take(most).take_while(|(new, old)| new == old).count();
        let new_changed = same..new.len() - same_at_end;
        let old_changed = same..old.len() - same_at_end;

        let common = new_changed.len().min(old_changed.len());
        for index in same..same + common {
            new[index].update(&old[index], commands, emitters[index]);
        }
        for index in old_changed.start + common..old_changed.end {
            commands.entity(emitters[index]).despawn();
        }
        for index in new_changed.start + common..new_changed.end {
            let emitter = new[index].spawn(commands, space);
            // It's the last child now. There can be children that aren't emitters, so its place is
            // found from the emitter that should follow it.
            if let Some(next) = emitters.get(same + common).copied() {
                commands.queue(move |world: &mut World| {
                    let children = world.get::<Children>(space);
                    let place = children.and_then(|children| children.iter().position(|c| c == next));
                    if let Some(place) = place {
                        world.entity_mut(space).insert_children(place, &[emitter]);
                    }
                });
            }
        }
        true
    }
}

impl EffectEmitter {
    fn transform(&self) -> Transform {
        Transform::from_translation(self.position.extend(0.0))
            .with_rotation(Quat::from_rotation_z(self.rotation))
    }

    fn spawn(&self, commands: &mut Commands, space: Entity) -> Entity {
        let mut emitter = commands.spawn((ChildOf(space), self.transform()));
        self.insert_settings(&mut emitter);
        emitter.id()
    }

    fn update(&self, old: &Self, commands: &mut Commands, entity: Entity) {
        let mut emitter = commands.entity(entity);
        self.insert_settings(&mut emitter);
        let placement = (&self.position, &self.rotation, &self.motion);
        if placement != (&old.position, &old.rotation, &old.motion) {
            emitter.insert(self.transform());
        }
    }

    /// Everything but the placement.
    fn insert_settings(&self, entity: &mut EntityCommands) {
        let Self { motion, def, .. } = self;
        entity.insert((def.emitter.clone(), def.modifiers.clone()));
        set_component(entity, def.tracks.clone());
        set_component(entity, def.attractor.clone());
        set_component(entity, (!motion.is_empty()).then(|| motion.clone()));
    }
}

fn set_component<T: Component>(entity: &mut EntityCommands, component: Option<T>) {
    match component {
        Some(component) => entity.insert(component),
        None => entity.remove::<T>(),
    };
}

#[derive(Default, TypePath)]
struct EffectLoader;

impl AssetLoader for EffectLoader {
    type Asset = ParticleEffect;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<ParticleEffect, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let file = EffectFile::from_ron(str::from_utf8(&bytes)?)?;

        let path = load_context.path().clone();
        let mut emitters = HashMap::new();
        for path in file.emitter_paths(&path)? {
            if !emitters.contains_key(&path) {
                // Also makes the effect reload when the emitter file changes.
                let bytes = load_context.read_asset_bytes(path.clone()).await?;
                let def = EmitterDef::from_ron(str::from_utf8(&bytes)?)
                    .map_err(|error| format!("{path}: {error}"))?;
                emitters.insert(path, def);
            }
        }
        file.build(
            &path,
            |path| Ok(emitters[path].clone()),
            |path| load_context.load(path),
        )
    }

    fn extensions(&self) -> &[&str] {
        &["effect.ron"]
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use bevy::asset::LoadState;

    use crate::render::{BlendMode, Playback};

    use super::*;

    /// Loads effects from `files` (path, contents) written to a temporary asset folder.
    fn load(folder: &str, files: &[(&str, &str)], path: &str) -> Result<ParticleEffect, String> {
        let root = std::env::temp_dir().join(format!("bevy_particle_{}_{folder}", std::process::id()));
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        let result = load_from(root.to_str().unwrap(), path);
        fs::remove_dir_all(root).unwrap();
        result
    }

    fn load_from(root: &str, path: &str) -> Result<ParticleEffect, String> {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin { file_path: root.to_string(), ..default() },
            ImagePlugin::default(),
            EffectPlugin,
        ));
        let server = app.world().resource::<AssetServer>().clone();
        let handle: Handle<ParticleEffect> = server.load(path.to_string());
        for _ in 0..1000 {
            app.update();
            match server.load_state(&handle) {
                LoadState::Loaded => {
                    let effects = app.world().resource::<Assets<ParticleEffect>>();
                    return Ok(effects.get(&handle).unwrap().clone());
                }
                LoadState::Failed(error) => return Err(error.to_string()),
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("{path} never loaded");
    }

    #[test]
    fn shipped_effects_load() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        let mut count = 0;
        for entry in fs::read_dir(format!("{root}/effects")).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if name.ends_with(".effect.ron") {
                load_from(root, &format!("effects/{name}")).unwrap();
                count += 1;
            }
        }
        assert!(count > 0);
    }

    #[test]
    fn emitters_come_from_files_or_inline() {
        let effect = load(
            "sources",
            &[
                (
                    "a/test.effect.ron",
                    r#"(space: (capacity: 8), emitters: [
                        (source: File("e.emitter.ron"), position: (1.0, 2.0)),
                        (source: File("/shared.emitter.ron")),
                        (source: Inline((emitter: (rate: 3.0)))),
                    ])"#,
                ),
                ("a/e.emitter.ron", "(emitter: (rate: 1.0))"),
                ("shared.emitter.ron", "(emitter: (rate: 2.0))"),
            ],
            "a/test.effect.ron",
        )
        .unwrap();
        let rates: Vec<_> = effect.emitters.iter().map(|e| e.def.emitter.rate).collect();
        assert_eq!(rates, [1.0, 2.0, 3.0]);
        assert_eq!(effect.emitters[0].position, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn render_settings_are_read() {
        let effect = load(
            "render",
            &[(
                "a/test.effect.ron",
                r#"(space: (capacity: 8), emitters: [], render: (
                    blend: Additive,
                    stretch: 0.5,
                    texture: (path: "smoke.png", columns: 4, playback: Fps(12.0)),
                ))"#,
            )],
            "a/test.effect.ron",
        )
        .unwrap();
        let render = effect.render;
        assert_eq!((render.blend, render.softness, render.stretch), (BlendMode::Additive, 0.0, 0.5));
        let texture = render.texture.unwrap();
        assert_eq!(texture.image.path().unwrap(), &"a/smoke.png".into());
        assert_eq!((texture.columns, texture.rows), (4, 1));
        assert_eq!(texture.playback, Playback::Fps(12.0));
    }

    #[test]
    fn bad_emitter_files_are_errors() {
        let effect = r#"(space: (capacity: 8), emitters: [(source: File("e.emitter.ron"))])"#;
        let effect = ("test.effect.ron", effect);
        let error = load("missing", &[effect], effect.0).unwrap_err();
        assert!(error.contains("e.emitter.ron"), "{error}");
        let typo = ("e.emitter.ron", "(emitter: (raet: 1.0))");
        let error = load("typo", &[effect, typo], effect.0).unwrap_err();
        assert!(error.contains("e.emitter.ron") && error.contains("raet"), "{error}");
    }

    #[test]
    fn spawns_the_space_with_its_emitters() {
        let mut world = World::new();
        let effect = ParticleEffect {
            space: ParticleSpace { capacity: 8 },
            render: default(),
            gravity: None,
            attractor: None,
            modifiers: default(),
            emitters: vec![
                EffectEmitter { position: Vec2::X, rotation: 0.0, motion: default(), def: default() },
                EffectEmitter { position: Vec2::Y, rotation: 0.0, motion: default(), def: default() },
            ],
        };
        let space = effect.spawn(&mut world.commands());
        world.flush();
        assert!(world.entity(space).contains::<ParticleSpace>());
        let mut emitters = world.query::<(&ParticleEmitter, &ChildOf)>();
        assert_eq!(emitters.iter(&world).filter(|(_, c)| c.parent() == space).count(), 2);

        world.despawn(space);
        assert_eq!(emitters.iter(&world).count(), 0);
    }

    #[test]
    fn spawned_effects_can_be_updated_in_place() {
        #[derive(Resource, Default)]
        struct AddedEmitters(usize);

        let file = r#"(
            space: (capacity: 8),
            gravity: (strength: 1.0, softening: 1.0, method: BruteForce, theta: 0.5),
            emitters: [
                (source: Inline((emitter: (rate: 1.0))), position: (1.0, 2.0)),
                (source: Inline((tracks: (rate: (clock: Time, keys: [(time: 0.0, value: 1.0)]))))),
            ],
        )"#;
        let file = EffectFile::from_ron(file).unwrap();
        let old = file.build(&"a".into(), |_| unreachable!(), |_| default()).unwrap();

        let mut app = App::new();
        app.init_resource::<AddedEmitters>().add_systems(
            Update,
            |added: Query<(), Added<ParticleEmitter>>, mut count: ResMut<AddedEmitters>| {
                count.0 += added.iter().count();
            },
        );
        let space = old.spawn(&mut app.world_mut().commands());
        app.update();
        let emitters = app.world().get::<Children>(space).unwrap().to_vec();

        let mut new = old.clone();
        new.gravity = None;
        new.emitters[0].def.emitter.rate = 5.0;
        new.emitters[0].def.tracks = Some(default());
        new.emitters[1].position = Vec2::X;
        new.emitters[1].def.tracks = None;
        // Something else moved the first one, whose placement doesn't change.
        app.world_mut().get_mut::<Transform>(emitters[0]).unwrap().translation.z = 7.0;
        assert!(new.update(&old, &mut app.world_mut().commands(), space, &emitters));
        app.update();

        let world = app.world();
        assert!(world.get::<Gravity>(space).is_none());
        assert_eq!(world.get::<ParticleEmitter>(emitters[0]).unwrap().rate, 5.0);
        assert!(world.get::<EmitterTracks>(emitters[0]).is_some());
        assert!(world.get::<EmitterTracks>(emitters[1]).is_none());
        let position = |entity| world.get::<Transform>(entity).unwrap().translation;
        assert_eq!(position(emitters[0]), Vec3::new(1.0, 2.0, 7.0));
        assert_eq!(position(emitters[1]), Vec3::X);
        // Emitters must not register again.
        assert_eq!(world.resource::<AddedEmitters>().0, 2);

        let mut bigger = old.clone();
        bigger.space.capacity = 9;
        assert!(!bigger.update(&old, &mut app.world_mut().commands(), space, &emitters));
    }

    #[test]
    fn emitters_around_added_and_removed_ones_keep_their_entities() {
        let effect = |rates: &[f32]| ParticleEffect {
            space: ParticleSpace { capacity: 8 },
            render: default(),
            gravity: None,
            attractor: None,
            modifiers: default(),
            emitters: rates
                .iter()
                .map(|rate| EffectEmitter {
                    position: Vec2::ZERO,
                    rotation: 0.0,
                    motion: default(),
                    def: EmitterDef {
                        emitter: ParticleEmitter { rate: *rate, ..default() },
                        ..default()
                    },
                })
                .collect(),
        };
        // (before, after, the old index that each emitter afterwards has the entity of)
        let cases: [(&[f32], &[f32], &[Option<usize>]); 9] = [
            (&[1.0, 2.0, 3.0], &[1.0, 3.0], &[Some(0), Some(2)]),
            (&[1.0, 2.0, 3.0], &[2.0, 3.0], &[Some(1), Some(2)]),
            (&[1.0, 3.0], &[1.0, 2.0, 3.0], &[Some(0), None, Some(1)]),
            (&[1.0, 2.0], &[9.0, 1.0, 2.0], &[None, Some(0), Some(1)]),
            (&[1.0], &[1.0, 2.0, 3.0], &[Some(0), None, None]),
            (&[4.0], &[2.0, 3.0, 4.0], &[None, None, Some(0)]),
            (&[], &[1.0], &[None]),
            (&[1.0], &[], &[]),
            (&[1.0, 1.0], &[1.0, 1.0, 1.0], &[Some(0), Some(1), None]),
        ];
        for (before, after, kept) in cases {
            let mut world = World::new();
            let (old, new) = (effect(before), effect(after));
            let space = old.spawn(&mut world.commands());
            world.flush();
            // Other children don't get in the way.
            let other = world.spawn_empty().id();
            world.entity_mut(space).insert_children(0, &[other]);
            let list = |world: &World| -> Vec<_> {
                let children = world.get::<Children>(space).unwrap();
                children.iter().filter(|child| *child != other).collect()
            };
            let entities = list(&world);
            assert!(new.update(&old, &mut world.commands(), space, &entities));
            world.flush();
            let result = list(&world);
            let rates: Vec<_> =
                result.iter().map(|e| world.get::<ParticleEmitter>(*e).unwrap().rate).collect();
            assert_eq!(rates, after, "{before:?}");
            for (entity, kept) in result.iter().zip(kept) {
                let old = entities.iter().position(|old| old == entity);
                assert_eq!(old, *kept, "{before:?} to {after:?}");
            }
        }
    }

    #[test]
    fn emitter_definitions_survive_saving_and_loading() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        for entry in load_from(root, "effects/showcase.effect.ron").unwrap().emitters {
            let saved = ron::ser::to_string(&entry.def).unwrap();
            let loaded: EmitterDef = ron::from_str(&saved).unwrap();
            assert_eq!(format!("{:?}", entry.def), format!("{loaded:?}"));
        }
    }

    #[test]
    fn emitter_tracks_are_read_and_spawned() {
        let effect = load(
            "tracks",
            &[(
                "test.effect.ron",
                r#"(space: (capacity: 8), emitters: [(
                    motion: (rotation: (clock: Time, keys: [(time: 0.0, value: 1.0)])),
                    source: Inline((
                        emitter: (spawn: Ring(radius: (from: 1.0, to: 2.0))),
                        tracks: (
                            rate: (clock: Time, repeat: Loop, keys: [(time: 0.0, value: 5.0)]),
                            spawn: Ring(radius: (clock: Time, keys: [(time: 0.0, value: (3.0, 4.0))])),
                        ),
                    )),
                )])"#,
            )],
            "test.effect.ron",
        )
        .unwrap();
        let def = &effect.emitters[0].def;
        assert_eq!(def.tracks.as_ref().unwrap().apply(&def.emitter, 0.0).rate, 5.0);
        let saved = ron::ser::to_string(def).unwrap();
        let loaded: EmitterDef = ron::from_str(&saved).unwrap();
        assert_eq!(format!("{def:?}"), format!("{loaded:?}"));

        let mut world = World::new();
        effect.spawn(&mut world.commands());
        world.flush();
        assert_eq!(world.query::<(&EmitterTracks, &EmitterMotion)>().iter(&world).count(), 1);
    }

    #[test]
    fn emitter_tracks_that_would_be_ignored_are_errors() {
        let key = "keys: [(time: 0.0, value: 1.0)]";
        let pair = "clock: Time, keys: [(time: 0.0, value: (1.0, 2.0))]";
        let cases = [
            (format!("source: Inline((tracks: (rate: (clock: Life, {key}))))"), "rate"),
            (format!("source: Inline(()), motion: (rotation: (clock: Seconds, {key}))"), "rotation"),
            (format!("source: Inline((tracks: (spawn: Rect(half_extents: ({pair})))))"), "area"),
            (
                format!("source: Inline((emitter: (lifetime: None), tracks: (lifetime: ({pair}))))"),
                "immortal",
            ),
        ];
        for (i, (emitter, expected)) in cases.iter().enumerate() {
            let effect = format!("(space: (capacity: 8), emitters: [({emitter})])");
            let file = ("test.effect.ron", effect.as_str());
            let error = load(&format!("ignored_tracks{i}"), &[file], file.0).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn track_keys_are_sorted_on_load() {
        let track: crate::track::Track<f32> =
            ron::from_str("(clock: Life, keys: [(time: 1.0, value: 2.0), (time: 0.0, value: 1.0)])")
                .unwrap();
        let sorted = crate::track::Track::over_life([(0.0, 1.0), (1.0, 2.0)]);
        assert_eq!(format!("{track:?}"), format!("{sorted:?}"));
    }
}
