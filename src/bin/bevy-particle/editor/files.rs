//! Keeps [`Documents`] in step with the asset files, and the scenes on screen with both.

use std::collections::BTreeMap;

use bevy::{
    asset::{io::Reader, AssetLoadFailedEvent, AssetLoader, LoadContext},
    prelude::*,
};
use bevy_particle::ParticleEffect;

use super::{documents::Documents, platform, Editor};
use crate::demos::{effect_path, Demos};

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SourceText>()
            .init_asset_loader::<SourceTextLoader>()
            .init_resource::<Documents>()
            .init_resource::<Files>()
            .add_systems(
                Update,
                (
                    open_files,
                    read_files,
                    #[cfg(target_arch = "wasm32")]
                    read_uploads,
                    keep_unsaved_changes,
                    update_effects,
                    #[cfg(target_arch = "wasm32")]
                    warn_before_leaving,
                )
                    .chain(),
            );
    }
}

/// A file as text. Effects and emitters are loaded as this in addition to
/// [`ParticleEffect`], which doesn't have everything that's in the files.
#[derive(Asset, TypePath)]
struct SourceText(String);

#[derive(Default, TypePath)]
struct SourceTextLoader;

impl AssetLoader for SourceTextLoader {
    type Asset = SourceText;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<SourceText, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(SourceText(String::from_utf8(bytes)?))
    }
}

#[derive(Resource, Default)]
pub struct Files {
    /// By path, for every file that's been asked for.
    handles: BTreeMap<String, Handle<SourceText>>,
    pub notice: Option<Notice>,
}

/// What the panel tells the user, until it's dismissed.
pub enum Notice {
    Info(String),
    Error(String),
    /// Saving these would lose their comments.
    Comments { scene: Option<String>, files: Vec<String> },
}

impl Files {
    fn error(&mut self, path: &str, error: impl std::fmt::Display) {
        error!("{path}: {error}");
        self.notice = Some(Notice::Error(format!("{path}: {error}")));
    }

    /// Saves the changed files of a scene, or of every scene. Asks first if comments would be
    /// lost, unless that's `confirmed`.
    pub fn save(&mut self, documents: &mut Documents, scene: Option<&str>, confirmed: bool) {
        self.notice = None;
        #[cfg(not(target_arch = "wasm32"))]
        for path in documents.dirty_files(scene) {
            // Something may have changed the file without us noticing.
            let result = platform::read(&path).and_then(|text| match text {
                Some(text) => documents.file_loaded(&path, text),
                None => Ok(()),
            });
            if let Err(error) = result {
                self.error(&path, error);
                return;
            }
        }
        // Asked about by the panel.
        if scene.is_some_and(|scene| documents.conflict_in(scene).is_some()) {
            return;
        }
        let files = documents.dirty_files(scene);
        let commented: Vec<_> =
            files.iter().filter(|path| documents.has_comments(path)).cloned().collect();
        if !confirmed && !commented.is_empty() {
            let scene = scene.map(Into::into);
            self.notice = Some(Notice::Comments { scene, files: commented });
            return;
        }
        for path in &files {
            let result = documents.to_save(path).and_then(|text| {
                platform::save(path, &text)?;
                Ok(text)
            });
            match result {
                Ok(text) => documents.mark_saved(path, text),
                Err(error) => {
                    self.error(path, error);
                    return;
                }
            }
        }
        if !files.is_empty() {
            let done = if cfg!(target_arch = "wasm32") { "Downloaded" } else { "Saved" };
            self.notice = Some(Notice::Info(format!("{done} {}", files.join(", "))));
        }
    }
}

/// Loads the files of every scene, from when the editor is first opened.
fn open_files(
    editor: Res<Editor>,
    demos: Res<Demos>,
    documents: Res<Documents>,
    assets: Res<AssetServer>,
    mut files: ResMut<Files>,
) {
    if !editor.open && files.handles.is_empty() {
        return;
    }
    for name in demos.names() {
        let scene = effect_path(name);
        for path in documents.files_of(&scene) {
            if !files.handles.contains_key(&path) {
                files.handles.insert(path.clone(), assets.load(path));
            }
        }
    }
}

fn read_files(
    mut events: MessageReader<AssetEvent<SourceText>>,
    mut failures: MessageReader<AssetLoadFailedEvent<SourceText>>,
    texts: Res<Assets<SourceText>>,
    mut files: ResMut<Files>,
    mut documents: ResMut<Documents>,
) {
    for failure in failures.read() {
        files.error(&failure.path.to_string(), &failure.error);
    }
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let path = files.handles.iter().find(|(_, handle)| handle.id() == *id);
        let (Some((path, _)), Some(text)) = (path, texts.get(*id)) else {
            continue;
        };
        let path = path.clone();
        if let Err(error) = documents.file_loaded(&path, text.0.clone()) {
            files.error(&path, error);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn read_uploads(mut files: ResMut<Files>, mut documents: ResMut<Documents>) {
    for (name, text) in platform::take_uploads() {
        // Files are told apart by name; downloads don't have folders.
        let suffix = format!("/{name}");
        let path = files.handles.keys().find(|path| path.ends_with(&suffix)).cloned();
        let Some(path) = path else {
            files.error(&name, "there's no scene or emitter file with this name");
            continue;
        };
        let result = text
            .map_err(BevyError::from)
            .and_then(|text| documents.replace(&path, &text));
        match result {
            // Keeps showing another file's error.
            Ok(()) if matches!(files.notice, Some(Notice::Error(_))) => {}
            Ok(()) => files.notice = Some(Notice::Info(format!("Loaded {path}"))),
            Err(error) => files.error(&path, error),
        }
    }
}

/// Hot reloading replaces effects by what's in their files; puts unsaved changes back.
fn keep_unsaved_changes(
    mut events: MessageReader<AssetEvent<ParticleEffect>>,
    demos: Res<Demos>,
    mut documents: ResMut<Documents>,
) {
    for event in events.read() {
        for name in demos.names() {
            let scene = effect_path(name);
            let modified = demos.handle(name).is_some_and(|handle| event.is_modified(handle.id()));
            if modified && !documents.dirty_files(Some(&scene)).is_empty() {
                documents.touch(&scene);
            }
        }
    }
}

/// Puts changed documents on screen. Replacing a shown effect respawns it.
fn update_effects(
    demos: Res<Demos>,
    assets: Res<AssetServer>,
    mut files: ResMut<Files>,
    mut documents: ResMut<Documents>,
    mut effects: ResMut<Assets<ParticleEffect>>,
) {
    let changed = documents.take_changed();
    for name in demos.names() {
        let scene = effect_path(name);
        if changed.is_empty() || !documents.files_of(&scene).iter().any(|f| changed.contains(f)) {
            continue;
        }
        let Some(handle) = demos.handle(name) else {
            continue;
        };
        match documents.build(&scene, |path| assets.load(path)) {
            Ok(effect) => {
                // Compared as text because NaNs don't equal themselves, and would be replaced
                // over and over.
                let current = effects.get(handle).map(|effect| format!("{effect:?}"));
                if current != Some(format!("{effect:?}")) {
                    let _ = effects.insert(handle.id(), effect);
                }
            }
            Err(error) => files.error(&scene, error),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn warn_before_leaving(documents: Res<Documents>) {
    platform::set_unsaved(!documents.dirty_files(None).is_empty());
}
