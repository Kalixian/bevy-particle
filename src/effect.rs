//! Particle effects stored in asset files.
//!
//! An `.effect.ron` file describes a whole [`ParticleEffect`]: a space and the emitters in it.
//! Emitters are written inline or kept in their own files, which effects can share. With file
//! watching on, saving either kind of file reloads the effects that use it.

use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    emitter::ParticleEmitter,
    gravity::Gravity,
    modifier::{Modifiers, TargetAttractor},
    render::{BlendMode, ParticleRender, ParticleTexture, Playback},
    space::ParticleSpace,
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
#[derive(Asset, TypePath, Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct EffectEmitter {
    /// Relative to the space.
    pub position: Vec2,
    /// Radians, counter-clockwise.
    pub rotation: f32,
    pub def: EmitterDef,
}

/// An emitter and the behaviors attached to it. Emitter files contain one of these.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmitterDef {
    pub emitter: ParticleEmitter,
    pub modifiers: Modifiers,
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

        for EffectEmitter { position, rotation, def } in &self.emitters {
            let mut emitter = commands.spawn((
                ChildOf(space),
                Transform::from_translation(position.extend(0.0))
                    .with_rotation(Quat::from_rotation_z(*rotation)),
                def.emitter.clone(),
                def.modifiers.clone(),
            ));
            if let Some(attractor) = &def.attractor {
                emitter.insert(attractor.clone());
            }
        }
        space
    }
}

/// An effect as written in its file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectFile {
    space: ParticleSpace,
    #[serde(default)]
    render: RenderFile,
    #[serde(default)]
    gravity: Option<Gravity>,
    #[serde(default)]
    attractor: Option<TargetAttractor>,
    #[serde(default)]
    modifiers: Modifiers,
    emitters: Vec<EmitterEntry>,
}

/// [`ParticleRender`] as written in a file.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RenderFile {
    blend: BlendMode,
    softness: f32,
    stretch: f32,
    texture: Option<TextureFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextureFile {
    /// Path of an image, found like [`EmitterSource::File`].
    path: String,
    #[serde(default = "one")]
    columns: u32,
    #[serde(default = "one")]
    rows: u32,
    #[serde(default)]
    playback: Playback,
}

fn one() -> u32 {
    1
}

impl RenderFile {
    fn load(self, load_context: &mut LoadContext<'_>) -> Result<ParticleRender, BevyError> {
        let texture = match self.texture {
            Some(TextureFile { path, columns, rows, playback }) => {
                let path = load_context.path().resolve_embed_str(&path)?;
                let image = load_context.load(path);
                Some(ParticleTexture { image, columns, rows, playback })
            }
            None => None,
        };
        Ok(ParticleRender {
            blend: self.blend,
            softness: self.softness,
            stretch: self.stretch,
            texture,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitterEntry {
    source: EmitterSource,
    #[serde(default)]
    position: Vec2,
    #[serde(default)]
    rotation: f32,
}

#[derive(Deserialize)]
enum EmitterSource {
    /// Path of an emitter file, relative to the effect file (or to the asset folder, with a
    /// leading `/`).
    File(String),
    Inline(Box<EmitterDef>),
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ron::error::SpannedError> {
    ron::Options::default()
        // Write `gravity: (..)` instead of `gravity: Some((..))`.
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_bytes(bytes)
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
        let file: EffectFile = parse(&bytes)?;

        let mut emitters = Vec::with_capacity(file.emitters.len());
        for EmitterEntry { source, position, rotation } in file.emitters {
            let def = match source {
                EmitterSource::Inline(def) => *def,
                EmitterSource::File(path) => {
                    let path = load_context.path().resolve_embed_str(&path)?;
                    // Also makes the effect reload when the emitter file changes.
                    let bytes = load_context.read_asset_bytes(path.clone()).await?;
                    parse(&bytes).map_err(|error| format!("{path}: {error}"))?
                }
            };
            emitters.push(EffectEmitter { position, rotation, def });
        }
        Ok(ParticleEffect {
            space: file.space,
            render: file.render.load(load_context)?,
            gravity: file.gravity,
            attractor: file.attractor,
            modifiers: file.modifiers,
            emitters,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["effect.ron"]
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use bevy::asset::LoadState;

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
                EffectEmitter { position: Vec2::X, rotation: 0.0, def: default() },
                EffectEmitter { position: Vec2::Y, rotation: 0.0, def: default() },
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
    fn emitter_definitions_survive_saving_and_loading() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        for entry in load_from(root, "effects/showcase.effect.ron").unwrap().emitters {
            let saved = ron::ser::to_string(&entry.def).unwrap();
            let loaded: EmitterDef = ron::from_str(&saved).unwrap();
            assert_eq!(format!("{:?}", entry.def), format!("{loaded:?}"));
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
