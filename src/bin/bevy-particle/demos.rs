//! Switches between the effects listed in `assets/demos.ron`.
//!
//! The left and right arrow keys step through the effects, number keys pick one of the first
//! nine, and `R` restarts the current one. With
//! `cargo run --features hot-reload`, saving a file restarts the effect it belongs to.

use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_particle::{ParticleEffect, ParticleSpace};
use serde::Deserialize;

use crate::input::{CaptureSystems, InputCapture};

const LIST_PATH: &str = "demos.ron";
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
        app.init_asset::<DemoList>()
            .init_asset_loader::<DemoListLoader>()
            .init_resource::<Demos>()
            .add_systems(Startup, load_list)
            .add_systems(Update, (read_list, pick_scene.after(CaptureSystems), respawn_scene).chain());
    }
}

/// The demos by name.
#[derive(Asset, TypePath, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DemoList {
    /// The scene shown first.
    start: String,
    scenes: Vec<String>,
}

impl DemoList {
    fn from_ron(bytes: &[u8]) -> Result<Self, BevyError> {
        let list: Self = ron::de::from_bytes(bytes)?;
        if !list.scenes.contains(&list.start) {
            return Err(format!("start scene \"{}\" isn't in the list", list.start).into());
        }
        Ok(list)
    }
}

fn effect_path(name: &str) -> String {
    format!("effects/{name}.effect.ron")
}

#[derive(Default, TypePath)]
struct DemoListLoader;

impl AssetLoader for DemoListLoader {
    type Asset = DemoList;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<DemoList, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        DemoList::from_ron(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        &["ron"]
    }
}

#[derive(Resource, Default)]
pub struct Demos {
    list: Handle<DemoList>,
    scenes: Vec<(String, Handle<ParticleEffect>)>,
    /// Index of the scene on screen.
    current: usize,
    /// The current scene should be (re)spawned once it's loaded.
    respawn: bool,
}

#[cfg(feature = "editor")]
impl Demos {
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.scenes.iter().map(|(name, _)| name.as_str())
    }

    /// Index of the scene on screen.
    pub fn current(&self) -> usize {
        self.current
    }

    pub fn show(&mut self, index: usize) {
        self.current = index;
        self.respawn = true;
    }

    pub fn restart(&mut self) {
        self.respawn = true;
    }
}

fn load_list(mut demos: ResMut<Demos>, assets: Res<AssetServer>) {
    demos.list = assets.load(LIST_PATH);
}

/// Loads the listed effects when the list is loaded or changes.
fn read_list(
    mut events: MessageReader<AssetEvent<DemoList>>,
    lists: Res<Assets<DemoList>>,
    assets: Res<AssetServer>,
    mut demos: ResMut<Demos>,
) {
    if !events
        .read()
        .any(|event| matches!(event, AssetEvent::Added { .. } | AssetEvent::Modified { .. }))
    {
        return;
    }
    // Read the remaining events too: `any` stops at the first match.
    events.clear();
    let Some(list) = lists.get(&demos.list) else {
        return;
    };
    // Stay on the current scene if it's still listed.
    let current = demos.scenes.get(demos.current).map_or(&list.start, |(name, _)| name);
    let index = list.scenes.iter().position(|name| name == current);
    let start = list.scenes.iter().position(|name| *name == list.start);
    let current = index.or(start).unwrap_or(0);
    demos.scenes = list
        .scenes
        .iter()
        .map(|name| (name.clone(), assets.load(effect_path(name))))
        .collect();
    demos.current = current;
    demos.respawn = true;
}

/// Picks or restarts a scene with the keyboard.
fn pick_scene(
    keys: Res<ButtonInput<KeyCode>>,
    capture: Res<InputCapture>,
    mut demos: ResMut<Demos>,
) {
    if capture.keyboard {
        return;
    }
    for (index, key) in SCENE_KEYS.iter().enumerate().take(demos.scenes.len()) {
        if keys.just_pressed(*key) {
            demos.current = index;
            demos.respawn = true;
        }
    }
    let count = demos.scenes.len();
    if count > 0 {
        let steps = [(KeyCode::ArrowRight, 1), (KeyCode::ArrowLeft, count - 1)];
        for (key, step) in steps {
            if keys.just_pressed(key) {
                demos.current = (demos.current + step) % count;
                demos.respawn = true;
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        demos.respawn = true;
    }
}

/// Replaces the space when a scene is picked, restarted, or reloaded from disk.
fn respawn_scene(
    mut commands: Commands,
    mut events: MessageReader<AssetEvent<ParticleEffect>>,
    effects: Res<Assets<ParticleEffect>>,
    mut demos: ResMut<Demos>,
    spaces: Query<Entity, With<ParticleSpace>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let Some((name, handle)) = demos.scenes.get(demos.current).cloned() else {
        return;
    };
    let reloaded = events
        .read()
        .filter(|event| event.is_modified(&handle))
        .count()
        > 0;
    demos.respawn |= reloaded;
    if !demos.respawn {
        return;
    }
    let Some(effect) = effects.get(&handle) else {
        return; // Still loading.
    };
    demos.respawn = false;

    for space in &spaces {
        // Despawning a space despawns its child emitters too.
        commands.entity(space).despawn();
    }
    let space = effect.spawn(&mut commands);
    commands.entity(space).insert(Name::new(name.clone()));
    info!("Showing scene \"{name}\"");
    window.title = format!("bevy-particle - {name}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_list_names_existing_effects() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/");
        let list = std::fs::read(format!("{root}{LIST_PATH}")).unwrap();
        for name in DemoList::from_ron(&list).unwrap().scenes {
            let path = format!("{root}{}", effect_path(&name));
            assert!(std::fs::exists(&path).unwrap(), "{path}");
        }
    }

    #[test]
    fn start_must_be_listed() {
        assert!(DemoList::from_ron(br#"(start: "a", scenes: ["b"])"#).is_err());
    }
}
