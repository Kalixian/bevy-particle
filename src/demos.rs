//! Demo scenes, loaded by name from `assets/demos.ron`.
//!
//! The library has two maps: reusable emitters, and scenes that combine space settings with
//! any number of those emitters. Number keys pick a scene (in name order) and `R` restarts it.
//! With `cargo run --features hot-reload`, saving the file restarts the current scene.

use std::collections::BTreeMap;

use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
    window::PrimaryWindow,
};
use serde::{Deserialize, Serialize};

use crate::{
    emitter::ParticleEmitter,
    gravity::Gravity,
    modifier::{Modifiers, MouseAttractor},
    space::ParticleSpace,
};

const LIBRARY_PATH: &str = "demos.ron";
const SCENE_KEYS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

pub struct DemosPlugin;

impl Plugin for DemosPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<DemoLibrary>()
            .init_asset_loader::<DemoLibraryLoader>()
            .init_resource::<CurrentScene>()
            .add_systems(Startup, load_library)
            .add_systems(Update, select_scene);
    }
}

/// Every demo emitter and scene, by name.
#[derive(Asset, TypePath, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoLibrary {
    /// The scene shown first.
    pub start: String,
    pub emitters: BTreeMap<String, EmitterDef>,
    pub scenes: BTreeMap<String, SceneDef>,
}

/// An emitter and the behaviors attached to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitterDef {
    #[serde(default)]
    pub emitter: ParticleEmitter,
    #[serde(default)]
    pub modifiers: Modifiers,
    #[serde(default)]
    pub mouse: Option<MouseAttractor>,
}

/// Space settings plus the emitters placed in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDef {
    pub space: ParticleSpace,
    #[serde(default)]
    pub gravity: Option<Gravity>,
    /// Pulls every particle in the scene; emitters can have their own too.
    #[serde(default)]
    pub mouse: Option<MouseAttractor>,
    /// Applied to every particle in the scene.
    #[serde(default)]
    pub modifiers: Modifiers,
    pub emitters: Vec<EmitterPlacement>,
}

/// One use of a library emitter in a scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitterPlacement {
    /// Name of an emitter in [`DemoLibrary::emitters`].
    pub name: String,
    #[serde(default)]
    pub position: Vec2,
}

impl DemoLibrary {
    fn options() -> ron::Options {
        // Write `gravity: (..)` instead of `gravity: Some((..))`.
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
    }

    /// Parses a library and checks that every name it refers to exists.
    pub fn from_ron(bytes: &[u8]) -> Result<Self, BevyError> {
        let library: Self = Self::options().from_bytes(bytes)?;
        if !library.scenes.contains_key(&library.start) {
            return Err(format!("start scene \"{}\" doesn't exist", library.start).into());
        }
        for (scene_name, scene) in &library.scenes {
            for EmitterPlacement { name, .. } in &scene.emitters {
                if !library.emitters.contains_key(name) {
                    return Err(
                        format!("scene \"{scene_name}\" uses unknown emitter \"{name}\"").into()
                    );
                }
            }
        }
        Ok(library)
    }

    /// Spawns the space of scene `name` with its emitters as children.
    fn spawn(&self, name: &str, commands: &mut Commands) {
        let Some(scene) = self.scenes.get(name) else {
            warn!("No demo scene named \"{name}\"");
            return;
        };
        let mut space = commands.spawn((
            Name::new(name.to_string()),
            scene.space.clone(),
            scene.modifiers.clone(),
        ));
        if let Some(gravity) = scene.gravity {
            space.insert(gravity);
        }
        if let Some(mouse) = &scene.mouse {
            space.insert(mouse.clone());
        }
        let space = space.id();

        for placement in &scene.emitters {
            let Some(def) = self.emitters.get(&placement.name) else {
                continue; // Ruled out by `from_ron`.
            };
            let mut emitter = commands.spawn((
                Name::new(placement.name.clone()),
                ChildOf(space),
                Transform::from_translation(placement.position.extend(0.0)),
                def.emitter.clone(),
                def.modifiers.clone(),
            ));
            if let Some(mouse) = &def.mouse {
                emitter.insert(mouse.clone());
            }
        }
    }
}

#[derive(Default, TypePath)]
struct DemoLibraryLoader;

impl AssetLoader for DemoLibraryLoader {
    type Asset = DemoLibrary;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<DemoLibrary, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        DemoLibrary::from_ron(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        &["ron"]
    }
}

/// The scene on screen. `name` is empty until the library loads.
#[derive(Resource, Default)]
struct CurrentScene {
    library: Handle<DemoLibrary>,
    name: String,
}

fn load_library(mut current: ResMut<CurrentScene>, assets: Res<AssetServer>) {
    current.library = assets.load(LIBRARY_PATH);
}

/// Respawns the whole space when a scene is picked, restarted (`R`), or reloaded from disk.
fn select_scene(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut events: MessageReader<AssetEvent<DemoLibrary>>,
    libraries: Res<Assets<DemoLibrary>>,
    mut current: ResMut<CurrentScene>,
    spaces: Query<Entity, With<ParticleSpace>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let mut respawn = events
        .read()
        .any(|event| matches!(event, AssetEvent::Added { .. } | AssetEvent::Modified { .. }));
    // Read the remaining events too: `any` stops at the first match.
    events.clear();

    let Some(library) = libraries.get(&current.library) else {
        return;
    };
    respawn |= keys.just_pressed(KeyCode::KeyR);
    for (key, name) in SCENE_KEYS.iter().zip(library.scenes.keys()) {
        if keys.just_pressed(*key) {
            current.name = name.clone();
            respawn = true;
        }
    }
    if !respawn {
        return;
    }
    if !library.scenes.contains_key(&current.name) {
        current.name = library.start.clone();
    }

    for space in &spaces {
        // Despawning a space despawns its child emitters too.
        commands.entity(space).despawn();
    }
    library.spawn(&current.name, &mut commands);
    info!("Showing scene \"{}\"", current.name);
    window.title = format!("bevy-particle - {}", current.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = include_str!("../assets/demos.ron");

    #[test]
    fn shipped_library_is_valid() {
        DemoLibrary::from_ron(SHIPPED.as_bytes()).unwrap();
    }

    #[test]
    fn library_survives_saving_and_loading() {
        let library = DemoLibrary::from_ron(SHIPPED.as_bytes()).unwrap();
        let saved = DemoLibrary::options()
            .to_string_pretty(&library, ron::ser::PrettyConfig::default())
            .unwrap();
        let loaded = DemoLibrary::from_ron(saved.as_bytes()).unwrap();
        assert_eq!(format!("{library:?}"), format!("{loaded:?}"));
    }

    #[test]
    fn missing_emitters_and_start_scenes_are_errors() {
        let unknown_emitter = r#"(start: "a", emitters: {}, scenes: {
            "a": (space: (capacity: 1), emitters: [(name: "nope")]),
        })"#;
        let error = DemoLibrary::from_ron(unknown_emitter.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("nope"), "{error}");
        let no_start = r#"(start: "a", emitters: {}, scenes: {})"#;
        assert!(DemoLibrary::from_ron(no_start.as_bytes()).is_err());
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
