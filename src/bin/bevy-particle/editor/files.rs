//! Keeps [`Documents`] in step with the asset files, and the scenes on screen with both.

use std::collections::BTreeMap;

use bevy::{
    asset::{io::Reader, AssetLoadFailedEvent, AssetLoader, LoadContext},
    prelude::*,
};
#[cfg(target_arch = "wasm32")]
use bevy_particle::{EffectFile, EmitterDef};
use bevy_particle::ParticleEffect;

#[cfg(target_arch = "wasm32")]
use super::documents::is_emitter_file;
use super::{documents::Documents, platform, Editor};
use crate::demos::{effect_path, Demos, LIST_PATH};

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
                    update_list,
                    update_effects,
                    #[cfg(not(target_arch = "wasm32"))]
                    close_window,
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
    /// The window is about to be closed with unsaved changes.
    #[cfg_attr(target_arch = "wasm32", expect(dead_code, reason = "the browser asks"))]
    Close,
}

impl Files {
    fn error(&mut self, path: &str, error: impl std::fmt::Display) {
        error!("{path}: {error}");
        self.notice = Some(Notice::Error(format!("{path}: {error}")));
    }

    /// Saves the changed files of a scene, or of every scene. Asks first if comments would be
    /// lost, unless that's `confirmed`. Returns whether everything was saved.
    pub fn save(
        &mut self,
        documents: &mut Documents,
        scene: Option<&str>,
        confirmed: bool,
    ) -> bool {
        self.notice = None;
        let to_save = |documents: &Documents| match scene {
            Some(scene) => documents.files_to_save(scene),
            None => {
                let mut files = documents.dirty_files(None);
                // Last, once everything on it is there.
                files.sort_by_key(|path| path == LIST_PATH);
                files
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        for path in to_save(documents) {
            // Something may have changed the file without us noticing.
            let result = platform::read(&path).and_then(|text| match text {
                Some(text) => documents.file_loaded(&path, text),
                None => Ok(()),
            });
            if let Err(error) = result {
                self.error(&path, error);
                return false;
            }
        }
        let files = to_save(documents);
        if let Some(path) = documents.conflict_in(&files) {
            // The panel asks about those of the scene on screen.
            if scene.is_none_or(|scene| documents.conflict_with(scene).is_none()) {
                let text = format!("{path} was changed outside the editor. Show a scene that uses it to settle this.");
                self.notice = Some(Notice::Error(text));
            }
            return false;
        }
        let commented: Vec<_> =
            files.iter().filter(|path| documents.has_comments(path)).cloned().collect();
        if !confirmed && !commented.is_empty() {
            let scene = scene.map(Into::into);
            self.notice = Some(Notice::Comments { scene, files: commented });
            return false;
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
                    return false;
                }
            }
        }
        if !files.is_empty() {
            let done = if cfg!(target_arch = "wasm32") { "Downloaded" } else { "Saved" };
            self.notice = Some(Notice::Info(format!("{done} {}", files.join(", "))));
        }
        true
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
    let scenes = demos.names().flat_map(|name| documents.files_of(&effect_path(name)));
    for path in scenes.chain([LIST_PATH.to_string()]) {
        if !files.handles.contains_key(&path) && documents.has_file(&path) {
            files.handles.insert(path.clone(), assets.load(path));
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
    let mut uploads = platform::take_uploads();
    // Scenes need their emitters, and the list needs its scenes.
    uploads.sort_by_key(|(name, _)| (name == LIST_PATH, !is_emitter_file(name)));
    for (name, text) in uploads {
        // Files are told apart by name; downloads don't have folders.
        let path = if name == LIST_PATH { name } else { format!("effects/{name}") };
        let result = text.map_err(BevyError::from).and_then(|text| {
            if documents.is_open(&path) {
                return documents.replace(&path, &text);
            }
            if is_emitter_file(&path) {
                return documents.add_emitter(&path, EmitterDef::from_ron(&text)?);
            }
            let scene = path.strip_prefix("effects/").and_then(|p| p.strip_suffix(".effect.ron"));
            let scene = scene.ok_or("it isn't a scene or emitter file")?;
            documents.add_scene(scene, EffectFile::from_ron(&text)?, None)
        });
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
            // Added, if it was loaded again after having been removed from the list.
            let modified = demos.handle(name).is_some_and(|handle| {
                event.is_modified(handle.id()) || event.is_added(handle.id())
            });
            if modified && !documents.dirty_files(Some(&scene)).is_empty() {
                documents.touch(&scene);
            }
        }
    }
}

/// Has the app show the scenes on the list as it's edited.
fn update_list(
    mut demos: ResMut<Demos>,
    assets: Res<AssetServer>,
    documents: Res<Documents>,
    mut effects: ResMut<Assets<ParticleEffect>>,
) {
    let Some(list) = documents.list() else {
        return;
    };
    // Scenes that have been saved for the first time, whose files can now be watched.
    let saved = |name: &str, handle: &Handle<ParticleEffect>| {
        handle.path().is_none() && documents.has_file(&effect_path(name))
    };
    let current = |name: &str| demos.handle(name).filter(|handle| !saved(name, handle)).cloned();
    if demos.names().eq(list.scenes.iter().map(String::as_str))
        && list.scenes.iter().all(|name| current(name).is_some())
    {
        return;
    }
    let scenes = list.scenes.iter().map(|name| {
        let path = effect_path(name);
        let handle = current(name).unwrap_or_else(|| {
            if documents.has_file(&path) {
                return assets.load(path);
            }
            match documents.build(&path, |path| assets.load(path)) {
                Ok(effect) => effects.add(effect),
                Err(_) => Handle::default(), // It wouldn't have been added.
            }
        });
        (name.clone(), handle)
    });
    let scenes = scenes.collect();
    demos.set_scenes(scenes);
}

/// Asks about unsaved changes before the window closes.
#[cfg(not(target_arch = "wasm32"))]
fn close_window(
    mut requests: MessageReader<bevy::window::WindowCloseRequested>,
    documents: Res<Documents>,
    mut editor: ResMut<Editor>,
    mut files: ResMut<Files>,
    mut exit: MessageWriter<AppExit>,
) {
    if requests.read().count() == 0 {
        return;
    }
    if documents.dirty_files(None).is_empty() {
        exit.write(AppExit::Success);
    } else {
        editor.open = true;
        files.notice = Some(Notice::Close);
    }
}

/// Puts changed documents on screen.
fn update_effects(
    mut demos: ResMut<Demos>,
    assets: Res<AssetServer>,
    mut files: ResMut<Files>,
    mut documents: ResMut<Documents>,
    mut effects: ResMut<Assets<ParticleEffect>>,
) {
    let changed = documents.take_changed();
    if changed.is_empty() {
        return;
    }
    let names: Vec<_> = demos.names().map(String::from).collect();
    for (index, name) in names.iter().enumerate() {
        let scene = effect_path(name);
        if !documents.files_of(&scene).iter().any(|f| changed.contains(f)) {
            continue;
        }
        let Some(handle) = demos.handle(name).cloned() else {
            continue;
        };
        match documents.build(&scene, |path| assets.load(path)) {
            Ok(effect) => {
                // Compared as text because NaNs don't equal themselves, and would be replaced
                // over and over.
                let current = effects.get(&handle).map(|effect| format!("{effect:?}"));
                if current != Some(format!("{effect:?}")) {
                    let _ = effects.insert(handle.id(), effect);
                    if index == demos.current() {
                        demos.update_live();
                    }
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
