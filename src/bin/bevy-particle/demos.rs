//! Switches between the effects listed in `assets/demos.ron`.
//!
//! The left and right arrow keys step through the effects, number keys pick one of the first
//! nine, and `R` restarts the current one. With
//! `cargo run --features hot-reload`, saving a change to a file restarts the effect
//! it belongs to.

use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_particle::{ParticleEffect, ParticleEmitter, ParticleSpace};
use serde::{Deserialize, Serialize};

use crate::input::{CaptureSystems, InputCapture};

pub const LIST_PATH: &str = "demos.ron";
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
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoList {
    /// The scene shown first.
    pub start: String,
    pub scenes: Vec<String>,
}

impl DemoList {
    pub fn from_ron(bytes: &[u8]) -> Result<Self, BevyError> {
        let list: Self = ron::de::from_bytes(bytes)?;
        if !list.scenes.contains(&list.start) {
            return Err(format!("start scene \"{}\" isn't in the list", list.start).into());
        }
        Ok(list)
    }

    #[cfg(feature = "editor")]
    pub fn to_ron(&self) -> Result<String, BevyError> {
        let config = ron::ser::PrettyConfig::new().new_line("\n");
        Ok(ron::ser::to_string_pretty(self, config)? + "\n")
    }
}

pub fn effect_path(name: &str) -> String {
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
    /// The current scene as it was spawned.
    shown: Option<ParticleEffect>,
    /// The next change to the current scene should be made without respawning it.
    live: bool,
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

    pub fn handle(&self, name: &str) -> Option<&Handle<ParticleEffect>> {
        self.scenes.iter().find(|(other, _)| other == name).map(|(_, handle)| handle)
    }

    /// Replaces the list of scenes. Stays on the current one, if it's still there.
    pub fn set_scenes(&mut self, scenes: Vec<(String, Handle<ParticleEffect>)>) {
        let name = self.scenes.get(self.current).map(|(name, _)| name);
        let index = scenes.iter().position(|(other, _)| Some(other) == name);
        self.respawn |= index.is_none();
        self.current = index.unwrap_or(self.current.min(scenes.len().saturating_sub(1)));
        self.scenes = scenes;
    }

    pub fn show(&mut self, index: usize) {
        self.current = index;
        self.respawn = true;
    }

    /// Has the next change to the current scene's effect leave existing particles in place, if
    /// possible.
    pub fn update_live(&mut self) {
        self.live = true;
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
    // Scenes that stay listed keep their effect, which may not have a file yet.
    let scenes = list.scenes.iter().map(|name| {
        let old = demos.scenes.iter().find(|(other, _)| other == name);
        let handle = old.map(|(_, handle)| handle.clone());
        (name.clone(), handle.unwrap_or_else(|| assets.load(effect_path(name))))
    });
    demos.scenes = scenes.collect();
    demos.current = current;
    // The scene on screen goes on if it's still listed.
    demos.respawn |= index.is_none() || demos.shown.is_none();
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
    spaces: Query<(Entity, Option<&Children>), With<ParticleSpace>>,
    emitters: Query<(), With<ParticleEmitter>>,
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
    // Saving a file without really changing it doesn't restart anything.
    let changed = reloaded && effects.get(&handle) != demos.shown.as_ref();
    if changed
        && demos.live
        && !demos.respawn
        && let (Some(effect), Some(shown)) = (effects.get(&handle), &demos.shown)
        && let Ok((space, children)) = spaces.single()
    {
        let children = children.into_iter().flatten().copied();
        let children: Vec<_> = children.filter(|child| emitters.contains(*child)).collect();
        if effect.update(shown, &mut commands, space, &children) {
            demos.shown = Some(effect.clone());
            demos.live = false;
            return;
        }
    }
    demos.respawn |= changed;
    if !demos.respawn {
        return;
    }
    let Some(effect) = effects.get(&handle) else {
        return; // Still loading.
    };
    demos.respawn = false;
    demos.live = false;
    demos.shown = Some(effect.clone());

    for (space, _) in &spaces {
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

    #[cfg(feature = "editor")]
    #[test]
    fn lists_survive_saving() {
        let list = DemoList { start: "b".into(), scenes: vec!["a".into(), "b".into()] };
        assert_eq!(DemoList::from_ron(list.to_ron().unwrap().as_bytes()).unwrap(), list);
    }

    #[cfg(feature = "editor")]
    #[test]
    fn the_current_scene_is_kept_if_it_stays_listed() {
        let scenes = |names: &[&str]| -> Vec<(String, Handle<ParticleEffect>)> {
            names.iter().map(|name| (name.to_string(), default())).collect()
        };
        let mut demos = Demos { scenes: scenes(&["a", "b", "c"]), current: 1, ..default() };
        demos.set_scenes(scenes(&["c", "a", "b"]));
        assert_eq!((demos.current, demos.respawn), (2, false));
        demos.set_scenes(scenes(&["c", "a"]));
        assert_eq!((demos.current, demos.respawn), (1, true));
    }

    #[test]
    fn start_must_be_listed() {
        assert!(DemoList::from_ron(br#"(start: "a", scenes: ["b"])"#).is_err());
    }
}
