//! The files being edited, as they are in memory and as they were last saved.

use std::collections::{BTreeMap, BTreeSet};

use bevy::{asset::AssetPath, prelude::*};
use bevy_particle::{
    EffectFile, EmitterDef, EmitterEntry, EmitterSource, ParticleEffect,
    ParticleSpace,
};

use crate::demos::{effect_path, DemoList, LIST_PATH};

/// What a kind of file contains.
pub trait Contents: Clone + PartialEq {
    fn parse(text: &str) -> Result<Self, BevyError>;
    fn to_text(&self) -> Result<String, BevyError>;
}

impl Contents for EffectFile {
    fn parse(text: &str) -> Result<Self, BevyError> {
        Self::from_ron(text)
    }

    fn to_text(&self) -> Result<String, BevyError> {
        self.to_ron()
    }
}

impl Contents for EmitterDef {
    fn parse(text: &str) -> Result<Self, BevyError> {
        Self::from_ron(text)
    }

    fn to_text(&self) -> Result<String, BevyError> {
        self.to_ron()
    }
}

impl Contents for DemoList {
    fn parse(text: &str) -> Result<Self, BevyError> {
        Self::from_ron(text.as_bytes())
    }

    fn to_text(&self) -> Result<String, BevyError> {
        self.to_ron()
    }
}

/// One file.
pub struct Document<T> {
    value: T,
    saved: T,
    /// It was made in the editor and hasn't been saved since.
    never_saved: bool,
    /// Whether the app could load it as an asset. On the web, saving doesn't change that.
    has_file: bool,
    /// The file's text, which `saved` is the meaning of.
    text: String,
    /// The file's new text, if it was changed by something else while there were unsaved changes.
    conflict: Option<String>,
}

impl<T: Contents> Document<T> {
    fn open(text: String) -> Result<Self, BevyError> {
        let value = T::parse(&text)?;
        let saved = value.clone();
        Ok(Self { value, saved, never_saved: false, has_file: true, text, conflict: None })
    }

    fn new(value: T) -> Self {
        Self {
            saved: value.clone(),
            value,
            never_saved: true,
            has_file: false,
            text: String::new(),
            conflict: None,
        }
    }

    fn is_dirty(&self) -> bool {
        self.never_saved || self.value != self.saved
    }

    /// Takes note of the file's text having changed. Returns whether that changed the value.
    fn file_changed(&mut self, text: String) -> Result<bool, BevyError> {
        if text == self.text {
            self.conflict = None;
            return Ok(false);
        }
        let saved = T::parse(&text)?;
        if self.is_dirty() && saved != self.value {
            self.conflict = Some(text);
            return Ok(false);
        }
        let changed = self.value != saved;
        self.value = saved.clone();
        self.saved = saved;
        (self.never_saved, self.has_file) = (false, true);
        self.text = text;
        self.conflict = None;
        Ok(changed)
    }

    /// Settles a conflict, either way.
    fn resolve(&mut self, keep_mine: bool) -> Result<(), BevyError> {
        if let Some(text) = &self.conflict {
            let saved = T::parse(text)?;
            if !keep_mine {
                self.value = saved.clone();
            }
            self.saved = saved;
            (self.never_saved, self.has_file) = (false, true);
            self.text = self.conflict.take().unwrap_or_default();
        }
        Ok(())
    }

    /// The text to save. Line endings stay as they are in the file.
    fn to_save(&self) -> Result<String, BevyError> {
        let text = self.value.to_text()?;
        Ok(if self.text.contains("\r\n") { text.replace('\n', "\r\n") } else { text })
    }

    fn mark_saved(&mut self, text: String) {
        self.saved = self.value.clone();
        self.never_saved = false;
        self.has_file |= cfg!(not(target_arch = "wasm32"));
        self.text = text;
        self.conflict = None;
    }
}

/// Calls a method of the document at a path, of any kind.
macro_rules! with_document {
    ($self:ident, $path:expr, $document:ident => $result:expr) => {
        if let Some($document) = $self.scenes.get_mut($path) {
            Some($result)
        } else if let Some($document) = $self.emitters.get_mut($path) {
            Some($result)
        } else if $path == LIST_PATH && let Some($document) = &mut $self.list {
            Some($result)
        } else {
            None
        }
    };
}

/// Most steps that can be undone.
const MAX_HISTORY: usize = 200;

/// A file's value from before or after a change.
enum Snapshot {
    Scene(EffectFile),
    Emitter(Box<EmitterDef>),
    List(DemoList),
}

struct Step {
    path: String,
    /// Changes of the same group are a single step; see [`Documents::end_group`].
    group: u64,
    snapshot: Snapshot,
}

/// Whether a value was changed. NaNs don't count as different from themselves.
fn differ<T: PartialEq + std::fmt::Debug>(a: &T, b: &T) -> bool {
    a != b && format!("{a:?}") != format!("{b:?}")
}

/// Every open file, by its path in the asset folder.
#[derive(Resource, Default)]
pub struct Documents {
    scenes: BTreeMap<String, Document<EffectFile>>,
    emitters: BTreeMap<String, Document<EmitterDef>>,
    list: Option<Document<DemoList>>,
    /// Files whose value changed since [`Self::take_changed`].
    changed: BTreeSet<String>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    group: u64,
}

/// The form of a path that [`Documents`] uses, whatever separators the platform has.
pub fn document_path(path: &AssetPath) -> String {
    path.to_string().replace('\\', "/")
}

pub fn is_emitter_file(path: &str) -> bool {
    path.ends_with(".emitter.ron")
}

impl Documents {
    /// Opens a file, or updates it if it was changed by something else.
    pub fn file_loaded(&mut self, path: &str, text: String) -> Result<(), BevyError> {
        match with_document!(self, path, document => document.file_changed(text.clone())) {
            Some(Ok(true)) => {
                // Undoing past this would undo the other program's change.
                self.forget_history(path);
                self.changed.insert(path.into());
            }
            Some(result) => {
                result?;
            }
            None if path == LIST_PATH => {
                self.list = Some(Document::open(text)?);
            }
            None if is_emitter_file(path) => {
                self.emitters.insert(path.into(), Document::open(text)?);
            }
            None => {
                self.scenes.insert(path.into(), Document::open(text)?);
            }
        }
        Ok(())
    }

    pub fn is_open(&self, path: &str) -> bool {
        self.scenes.contains_key(path)
            || self.emitters.contains_key(path)
            || (path == LIST_PATH && self.list.is_some())
    }

    /// Whether the app could load this as an asset. Unless it's known not to exist, it could.
    pub fn has_file(&self, path: &str) -> bool {
        self.scenes.get(path).is_none_or(|document| document.has_file)
            && self.emitters.get(path).is_none_or(|document| document.has_file)
    }

    pub fn list(&self) -> Option<&DemoList> {
        self.list.as_ref().map(|document| &document.value)
    }

    /// Changes the list of scenes, as something that can be undone.
    pub fn edit_list(&mut self, list: DemoList) {
        if let Some(old) = self.list()
            && *old != list
        {
            let old = old.clone();
            self.record(LIST_PATH, Snapshot::List(old));
            self.set(LIST_PATH, Snapshot::List(list));
        }
    }

    /// Adds a scene without a file to the list, after the scene named `after` or at the end.
    pub fn add_scene(
        &mut self,
        name: &str,
        file: EffectFile,
        after: Option<&str>,
    ) -> Result<(), BevyError> {
        check_name(name)?;
        let path = effect_path(name);
        if self.is_open(&path) || self.list().is_some_and(|list| list.scenes.contains(&name.into()))
        {
            return Err(format!("there's a scene called {name} already").into());
        }
        // Refuse what can't be shown.
        self.build_file(&path, &file, |_| Handle::default())?;
        let mut list = self.list().ok_or("the list of scenes isn't open")?.clone();
        let place = list.scenes.iter().position(|other| Some(other.as_str()) == after);
        list.scenes.insert(place.map_or(list.scenes.len(), |place| place + 1), name.into());
        self.scenes.insert(path.clone(), Document::new(file));
        self.changed.insert(path);
        self.edit_list(list);
        self.end_group();
        Ok(())
    }

    /// Takes a scene off the list. Its file stays.
    pub fn remove_scene(&mut self, name: &str) {
        let Some(mut list) = self.list().cloned() else {
            return;
        };
        list.scenes.retain(|other| other != name);
        let Some(first) = list.scenes.first() else {
            return; // There has to be one.
        };
        if list.start == name {
            list.start = first.clone();
        }
        self.edit_list(list);
        self.end_group();
    }

    /// Opens an emitter file that doesn't exist yet.
    pub fn add_emitter(&mut self, path: &str, def: EmitterDef) -> Result<(), BevyError> {
        if self.is_open(path) {
            return Err(format!("{path} exists already").into());
        }
        if let Some(tracks) = &def.tracks {
            tracks.check(&def.emitter)?;
        }
        self.emitters.insert(path.into(), Document::new(def));
        Ok(())
    }

    pub fn emitter_files(&self) -> impl Iterator<Item = &str> {
        self.emitters.keys().map(String::as_str)
    }

    /// Replaces a file's value with that of `text`, as an unsaved change.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code, reason = "web only"))]
    pub fn replace(&mut self, path: &str, text: &str) -> Result<(), BevyError> {
        if self.scenes.contains_key(path) {
            let file = EffectFile::from_ron(text)?;
            // Refuse what can't be shown.
            self.build_file(path, &file, |_| Handle::default())?;
            self.edit_scene(path, file);
        } else if self.emitters.contains_key(path) {
            let def = EmitterDef::from_ron(text)?;
            if let Some(tracks) = &def.tracks {
                tracks.check(&def.emitter)?;
            }
            self.edit_emitter(path, def);
        } else if path == LIST_PATH && self.list.is_some() {
            self.edit_list(DemoList::from_ron(text.as_bytes())?);
        } else {
            return Err(format!("{path} isn't open").into());
        }
        self.end_group();
        Ok(())
    }

    pub fn scene(&self, path: &str) -> Option<&EffectFile> {
        self.scenes.get(path).map(|document| &document.value)
    }

    pub fn emitter(&self, path: &str) -> Option<&EmitterDef> {
        self.emitters.get(path).map(|document| &document.value)
    }

    /// Changes a scene, as something that can be undone.
    pub fn edit_scene(&mut self, path: &str, file: EffectFile) {
        if let Some(old) = self.scene(path)
            && differ(old, &file)
        {
            let old = old.clone();
            self.record(path, Snapshot::Scene(old));
            self.set(path, Snapshot::Scene(file));
        }
    }

    /// Changes an emitter file, as something that can be undone.
    pub fn edit_emitter(&mut self, path: &str, def: EmitterDef) {
        if let Some(old) = self.emitter(path)
            && differ(old, &def)
        {
            let old = old.clone();
            self.record(path, Snapshot::Emitter(Box::new(old)));
            self.set(path, Snapshot::Emitter(Box::new(def)));
        }
    }

    /// Has later changes be undone separately from earlier ones. Until then, changes to the same
    /// file, such as those of a drag, are one step.
    pub fn end_group(&mut self) {
        self.group += 1;
    }

    fn record(&mut self, path: &str, snapshot: Snapshot) {
        self.redo.retain(|step| step.path != path);
        let group = self.group;
        if self.undo.iter().any(|step| step.group == group && step.path == path) {
            return;
        }
        self.undo.push(Step { path: path.into(), group, snapshot });
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
    }

    fn set(&mut self, path: &str, snapshot: Snapshot) {
        match snapshot {
            Snapshot::Scene(file) => {
                if let Some(document) = self.scenes.get_mut(path) {
                    document.value = file;
                }
            }
            Snapshot::Emitter(def) => {
                if let Some(document) = self.emitters.get_mut(path) {
                    document.value = *def;
                }
            }
            Snapshot::List(list) => {
                if let Some(document) = &mut self.list {
                    document.value = list;
                }
            }
        }
        self.changed.insert(path.into());
    }

    fn snapshot(&self, path: &str) -> Option<Snapshot> {
        if path == LIST_PATH {
            return self.list().cloned().map(Snapshot::List);
        }
        let scene = self.scene(path).cloned().map(Snapshot::Scene);
        scene.or_else(|| self.emitter(path).cloned().map(|def| Snapshot::Emitter(Box::new(def))))
    }

    fn forget_history(&mut self, path: &str) {
        self.undo.retain(|step| step.path != path);
        self.redo.retain(|step| step.path != path);
    }

    /// The files that undoing affects while a scene is on screen.
    fn undo_scope(&self, scene: &str) -> Vec<String> {
        let mut files = self.files_of(scene);
        files.push(LIST_PATH.into());
        files
    }

    pub fn can_undo(&self, scene: &str) -> bool {
        let files = self.undo_scope(scene);
        self.undo.iter().any(|step| files.contains(&step.path))
    }

    pub fn can_redo(&self, scene: &str) -> bool {
        let files = self.undo_scope(scene);
        self.redo.iter().any(|step| files.contains(&step.path))
    }

    /// Undoes the last step in the files of a scene, or in the list of scenes.
    pub fn undo(&mut self, scene: &str) {
        let files = self.undo_scope(scene);
        if let Some(index) = self.undo.iter().rposition(|step| files.contains(&step.path)) {
            let Step { path, group, snapshot } = self.undo.remove(index);
            if let Some(current) = self.snapshot(&path) {
                self.redo.push(Step { path: path.clone(), group, snapshot: current });
            }
            self.set(&path, snapshot);
        }
        self.end_group();
    }

    pub fn redo(&mut self, scene: &str) {
        let files = self.undo_scope(scene);
        if let Some(index) = self.redo.iter().rposition(|step| files.contains(&step.path)) {
            let Step { path, group, snapshot } = self.redo.remove(index);
            if let Some(current) = self.snapshot(&path) {
                self.undo.push(Step { path: path.clone(), group, snapshot: current });
            }
            self.set(&path, snapshot);
        }
        self.end_group();
    }

    /// Every image that the scenes use, as paths from the asset folder.
    pub fn textures(&self) -> BTreeSet<String> {
        self.scenes
            .iter()
            .filter_map(|(scene, document)| {
                let texture = document.value.render.texture.as_ref()?;
                texture_path(scene, &texture.path)
            })
            .collect()
    }

    /// The emitter files a scene uses.
    pub fn emitters_of(&self, scene: &str) -> BTreeSet<String> {
        let Some(document) = self.scenes.get(scene) else {
            return default();
        };
        let path = AssetPath::parse(scene);
        let paths = document.value.emitter_paths(&path).unwrap_or_default();
        paths.iter().map(document_path).collect()
    }

    /// A scene and the emitter files it uses.
    pub fn files_of(&self, scene: &str) -> Vec<String> {
        let mut files = vec![scene.to_string()];
        files.extend(self.emitters_of(scene));
        files
    }

    /// The scenes that show what's in a file.
    pub fn scenes_using(&self, path: &str) -> Vec<String> {
        self.scenes
            .keys()
            .filter(|scene| *scene == path || self.emitters_of(scene).contains(path))
            .cloned()
            .collect()
    }

    /// Whether a file has unsaved changes.
    pub fn is_dirty(&self, path: &str) -> bool {
        self.scenes.get(path).is_some_and(Document::is_dirty)
            || self.emitters.get(path).is_some_and(Document::is_dirty)
            || (path == LIST_PATH && self.list.as_ref().is_some_and(Document::is_dirty))
    }

    /// The files with unsaved changes, out of those of a scene or out of all.
    pub fn dirty_files(&self, scene: Option<&str>) -> Vec<String> {
        let files: BTreeSet<_> = match scene {
            Some(scene) => self.files_of(scene).into_iter().collect(),
            None => self.used_files(),
        };
        files.into_iter().filter(|path| self.is_dirty(path)).collect()
    }

    /// Every file but those that exist only in the editor and aren't part of anything (any more).
    fn used_files(&self) -> BTreeSet<String> {
        let listed = |scene: &String| {
            self.list().is_none_or(|list| list.scenes.iter().any(|s| effect_path(s) == *scene))
        };
        let scenes = self.scenes.iter().filter(|(path, scene)| !scene.never_saved || listed(path));
        let mut files: BTreeSet<_> = scenes.flat_map(|(path, _)| self.files_of(path)).collect();
        let existing = self.emitters.iter().filter(|(_, document)| !document.never_saved);
        files.extend(existing.map(|(path, _)| path.clone()));
        files.insert(LIST_PATH.into());
        files
    }

    /// What saving a scene should save. If the list has changed, that too, and with it the new
    /// scenes on it, so that it doesn't name any that don't exist.
    pub fn files_to_save(&self, scene: &str) -> Vec<String> {
        let mut files = self.dirty_files(Some(scene));
        if self.is_dirty(LIST_PATH) {
            for name in self.list().map_or(&[][..], |list| &list.scenes) {
                let other = effect_path(name);
                if self.scenes.get(&other).is_some_and(|document| document.never_saved) {
                    files.extend(self.dirty_files(Some(&other)));
                }
            }
            files.sort();
            files.dedup();
            // Last, once everything on it is there.
            files.push(LIST_PATH.into());
        }
        files
    }

    /// The first of some files that was also changed by something else.
    pub fn conflict_in(&self, files: &[String]) -> Option<String> {
        let conflicted = |path: &&String| {
            self.scenes.get(*path).is_some_and(|document| document.conflict.is_some())
                || self.emitters.get(*path).is_some_and(|document| document.conflict.is_some())
                || (*path == LIST_PATH
                    && self.list.as_ref().is_some_and(|document| document.conflict.is_some()))
        };
        files.iter().find(conflicted).cloned()
    }

    /// Like [`Self::conflict_in`], for what's on screen with a scene.
    pub fn conflict_with(&self, scene: &str) -> Option<String> {
        self.conflict_in(&self.undo_scope(scene))
    }

    pub fn resolve(&mut self, path: &str, keep_mine: bool) -> Result<(), BevyError> {
        self.changed.insert(path.into());
        if !keep_mine {
            self.forget_history(path);
        }
        with_document!(self, path, document => document.resolve(keep_mine)).unwrap_or(Ok(()))
    }

    /// Drops a file's unsaved changes. Can be undone.
    pub fn revert(&mut self, path: &str) {
        if let Some(document) = self.scenes.get(path).filter(|document| !document.never_saved) {
            self.edit_scene(path, document.saved.clone());
        } else if let Some(document) =
            self.emitters.get(path).filter(|document| !document.never_saved)
        {
            self.edit_emitter(path, document.saved.clone());
        }
    }

    /// Whether [`Self::revert`] would do anything.
    pub fn can_revert(&self, path: &str) -> bool {
        self.scenes.get(path).is_some_and(|d| !d.never_saved && d.is_dirty())
            || self.emitters.get(path).is_some_and(|d| !d.never_saved && d.is_dirty())
    }

    /// Whether saving would lose comments, which aren't part of a file's value.
    pub fn has_comments(&self, path: &str) -> bool {
        let scene = self.scenes.get(path).map(|document| &document.text);
        let list = self.list.as_ref().filter(|_| path == LIST_PATH);
        let text = scene
            .or(self.emitters.get(path).map(|document| &document.text))
            .or(list.map(|document| &document.text));
        text.is_some_and(|text| has_comments(text))
    }

    pub fn to_save(&mut self, path: &str) -> Result<String, BevyError> {
        with_document!(self, path, document => document.to_save())
            .unwrap_or_else(|| Err(format!("{path} isn't open").into()))
    }

    pub fn mark_saved(&mut self, path: &str, text: String) {
        with_document!(self, path, document => document.mark_saved(text));
    }

    /// Has [`Self::take_changed`] report a file again.
    pub fn touch(&mut self, path: &str) {
        self.changed.insert(path.into());
    }

    pub fn take_changed(&mut self) -> BTreeSet<String> {
        std::mem::take(&mut self.changed)
    }

    /// The effect that a scene describes, with unsaved changes.
    pub fn build(
        &self,
        scene: &str,
        image: impl FnMut(AssetPath<'static>) -> Handle<Image>,
    ) -> Result<ParticleEffect, BevyError> {
        let file = self.scene(scene).ok_or_else(|| format!("{scene} isn't open"))?;
        self.build_file(scene, file, image)
    }

    fn build_file(
        &self,
        scene: &str,
        file: &EffectFile,
        image: impl FnMut(AssetPath<'static>) -> Handle<Image>,
    ) -> Result<ParticleEffect, BevyError> {
        let emitter = |path: &AssetPath<'static>| {
            let path = document_path(path);
            match self.emitters.get(&path) {
                Some(document) => Ok(document.value.clone()),
                None => Err(format!("{path} isn't open").into()),
            }
        };
        file.build(&AssetPath::parse(scene), emitter, image)
    }
}

/// Names of scenes and emitter files become part of file names.
pub fn check_name(name: &str) -> Result<(), BevyError> {
    let fits = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_');
    if name.is_empty() || name.len() > 60 || !name.chars().all(fits) {
        return Err("names are made of a-z, 0-9, - and _".into());
    }
    Ok(())
}

/// The first of `name`, `name2`, `name3`.. that isn't taken.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let numbered = (2..).map(|number| format!("{name}{number}"));
    std::iter::once(name.to_string()).chain(numbered).find(|name| !taken(name)).unwrap()
}

/// A scene to start from.
pub fn new_scene() -> EffectFile {
    EffectFile {
        space: ParticleSpace { capacity: 10_000 },
        render: default(),
        gravity: None,
        attractor: None,
        modifiers: default(),
        emitters: vec![new_emitter()],
    }
}

pub fn new_emitter() -> EmitterEntry {
    EmitterEntry {
        position: Vec2::ZERO,
        rotation: 0.0,
        motion: default(),
        source: EmitterSource::Inline(default()),
    }
}

/// Where an image is, from the asset folder, if a scene refers to it as `texture`.
pub fn texture_path(scene: &str, texture: &str) -> Option<String> {
    let path = AssetPath::parse(scene).resolve_embed_str(texture).ok()?;
    Some(document_path(&path))
}

/// Whether there's a comment in some RON.
fn has_comments(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '/' if matches!(chars.peek(), Some('/' | '*')) => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "effects/a.effect.ron";
    const EMITTER: &str = "effects/e.emitter.ron";

    fn scene(capacity: u32) -> String {
        format!("(space: (capacity: {capacity}), emitters: [(source: File(\"e.emitter.ron\"))])")
    }

    fn emitter(rate: f32) -> String {
        format!("(emitter: (rate: {rate:?}))")
    }

    fn open() -> Documents {
        let mut documents = Documents::default();
        documents.file_loaded(SCENE, scene(8)).unwrap();
        documents.file_loaded(EMITTER, emitter(1.0)).unwrap();
        documents
    }

    fn rate(documents: &Documents) -> f32 {
        documents.build(SCENE, |_| default()).unwrap().emitters[0].def.emitter.rate
    }

    #[test]
    fn scenes_use_the_open_emitter_files() {
        let mut documents = open();
        assert_eq!(documents.files_of(SCENE), [SCENE, EMITTER]);
        assert_eq!(documents.scenes_using(EMITTER), [SCENE]);
        assert_eq!(rate(&documents), 1.0);

        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        assert_eq!(rate(&documents), 5.0);
        assert_eq!(documents.dirty_files(Some(SCENE)), [EMITTER]);
        assert_eq!(documents.take_changed().into_iter().collect::<Vec<_>>(), [EMITTER]);

        documents.revert(EMITTER);
        assert_eq!(rate(&documents), 1.0);
        assert!(documents.dirty_files(None).is_empty());
    }

    #[test]
    fn saving_makes_changes_clean_and_our_own_save_changes_nothing() {
        let mut documents = open();
        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        documents.take_changed();
        let text = documents.to_save(EMITTER).unwrap();
        documents.mark_saved(EMITTER, text.clone());
        assert!(!documents.is_dirty(EMITTER));
        // The file watcher reports the save.
        documents.file_loaded(EMITTER, text).unwrap();
        assert!(documents.take_changed().is_empty() && documents.conflict_with(SCENE).is_none());
    }

    #[test]
    fn outside_changes_are_adopted_unless_there_are_unsaved_ones() {
        let mut documents = open();
        documents.file_loaded(EMITTER, emitter(2.0)).unwrap();
        assert_eq!((rate(&documents), documents.is_dirty(EMITTER)), (2.0, false));

        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        documents.file_loaded(EMITTER, emitter(3.0)).unwrap();
        assert_eq!(rate(&documents), 5.0);
        assert_eq!(documents.conflict_with(SCENE).as_deref(), Some(EMITTER));

        documents.resolve(EMITTER, true).unwrap();
        assert!(documents.conflict_with(SCENE).is_none() && documents.is_dirty(EMITTER));
        // Mine is kept, but reverting now gets theirs.
        assert_eq!(rate(&documents), 5.0);
        documents.revert(EMITTER);
        assert_eq!(rate(&documents), 3.0);
    }

    #[test]
    fn conflicts_can_be_settled_with_the_other_version() {
        let mut documents = open();
        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        documents.file_loaded(EMITTER, emitter(3.0)).unwrap();
        documents.resolve(EMITTER, false).unwrap();
        assert_eq!((rate(&documents), documents.is_dirty(EMITTER)), (3.0, false));
    }

    #[test]
    fn broken_files_change_nothing() {
        let mut documents = open();
        assert!(documents.file_loaded(EMITTER, "(emitter: (raet".into()).is_err());
        assert!(documents.replace(SCENE, "(").is_err());
        let missing = "(space: (capacity: 8), emitters: [(source: File(\"x.emitter.ron\"))])";
        assert!(documents.replace(SCENE, missing).is_err());
        assert_eq!(rate(&documents), 1.0);
        assert!(documents.dirty_files(None).is_empty());
    }

    #[test]
    fn a_group_of_changes_is_undone_at_once() {
        let mut documents = open();
        let edit = |documents: &mut Documents, rate: f32| {
            documents.edit_emitter(EMITTER, EmitterDef::from_ron(&emitter(rate)).unwrap());
        };
        edit(&mut documents, 2.0);
        edit(&mut documents, 3.0);
        documents.end_group();
        edit(&mut documents, 4.0);
        edit(&mut documents, 4.0);
        documents.end_group();

        let mut rates = Vec::new();
        for _ in 0..3 {
            documents.undo(SCENE);
            rates.push(rate(&documents));
        }
        assert_eq!(rates, [3.0, 1.0, 1.0]);
        assert!(!documents.is_dirty(EMITTER) && !documents.can_undo(SCENE));
        for _ in 0..3 {
            documents.redo(SCENE);
            rates.push(rate(&documents));
        }
        assert_eq!(rates[3..], [3.0, 4.0, 4.0]);

        documents.undo(SCENE);
        edit(&mut documents, 9.0);
        assert!(!documents.can_redo(SCENE));
        documents.undo(SCENE);
        assert_eq!(rate(&documents), 3.0);
    }

    #[test]
    fn undoing_stays_in_the_files_of_the_scene() {
        const OTHER: &str = "effects/b.effect.ron";
        let other = |capacity| format!("(space: (capacity: {capacity}), emitters: [])");
        let mut documents = open();
        documents.file_loaded(OTHER, other(8)).unwrap();
        documents.replace(SCENE, &scene(9)).unwrap();
        documents.replace(OTHER, &other(7)).unwrap();
        documents.undo(SCENE);
        assert!(!documents.is_dirty(SCENE) && documents.is_dirty(OTHER));
        // Nothing left to undo here.
        documents.undo(SCENE);
        assert!(documents.is_dirty(OTHER));
    }

    #[test]
    fn reverting_can_be_undone_but_outside_changes_cant() {
        let mut documents = open();
        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        documents.revert(EMITTER);
        documents.undo(SCENE);
        assert_eq!(rate(&documents), 5.0);
        documents.undo(SCENE);
        documents.file_loaded(EMITTER, emitter(2.0)).unwrap();
        documents.undo(SCENE);
        documents.redo(SCENE);
        assert_eq!(rate(&documents), 2.0);
    }

    fn open_with_list() -> Documents {
        let mut documents = open();
        let list = "(start: \"a\", scenes: [\"a\"]) // demos";
        documents.file_loaded(LIST_PATH, list.into()).unwrap();
        documents
    }

    fn names(documents: &Documents) -> Vec<&str> {
        documents.list().unwrap().scenes.iter().map(String::as_str).collect()
    }

    #[test]
    fn new_scenes_are_saved_before_the_list() {
        let mut documents = open_with_list();
        documents.add_scene("z", new_scene(), Some("a")).unwrap();
        documents.add_scene("b", new_scene(), Some("a")).unwrap();
        assert_eq!(names(&documents), ["a", "b", "z"]);
        let z = effect_path("z");
        assert!(!documents.has_file(&z) && documents.is_dirty(&z) && !documents.can_revert(&z));
        assert!(documents.build(&z, |_| default()).is_ok());
        // From any scene.
        let files = documents.files_to_save(SCENE);
        assert_eq!(files, [effect_path("b"), z.clone(), LIST_PATH.into()]);
        assert!(documents.has_comments(LIST_PATH));

        for path in files {
            let text = documents.to_save(&path).unwrap();
            documents.mark_saved(&path, text);
        }
        assert!(documents.dirty_files(None).is_empty() && documents.has_file(&z));
    }

    #[test]
    fn undone_scenes_arent_unsaved_changes() {
        let mut documents = open_with_list();
        documents.add_scene("b", new_scene(), None).unwrap();
        documents.undo(SCENE);
        assert_eq!(names(&documents), ["a"]);
        assert!(documents.dirty_files(None).is_empty());
        documents.redo(SCENE);
        assert_eq!(names(&documents), ["a", "b"]);
        assert_eq!(documents.dirty_files(None).len(), 2);
    }

    #[test]
    fn scene_names_must_be_free_and_usable() {
        let mut documents = open_with_list();
        for name in ["a", "", "A", "a/b", "..", "a b", "a.b"] {
            assert!(documents.add_scene(name, new_scene(), None).is_err(), "{name}");
        }
        assert_eq!(free_name("a", |name| ["a", "a2"].contains(&name)), "a3");
        assert_eq!(free_name("b", |_| false), "b");
    }

    #[test]
    fn removing_scenes_keeps_the_list_usable() {
        let mut documents = open_with_list();
        documents.remove_scene("a");
        assert_eq!(names(&documents), ["a"]);
        documents.add_scene("b", new_scene(), None).unwrap();
        documents.replace(SCENE, &scene(9)).unwrap();
        documents.remove_scene("a");
        // Its changes aren't forgotten.
        assert!(documents.dirty_files(None).contains(&SCENE.to_string()));
        let list = documents.list().unwrap();
        assert_eq!((list.start.as_str(), &list.scenes[..]), ("b", &["b".to_string()][..]));
        assert!(DemoList::from_ron(list.to_ron().unwrap().as_bytes()).is_ok());
    }

    #[test]
    fn a_new_files_name_being_taken_is_a_conflict() {
        let mut documents = open_with_list();
        documents.add_scene("b", new_scene(), None).unwrap();
        let b = effect_path("b");
        // As saving finds it.
        documents.file_loaded(&b, scene(5)).unwrap();
        assert_eq!(documents.conflict_with(&b), Some(b.clone()));
        documents.resolve(&b, true).unwrap();
        assert!(documents.is_dirty(&b));
        assert_eq!(documents.scene(&b).unwrap().space.capacity, 10_000);
    }

    #[test]
    fn emitter_files_that_nothing_uses_dont_need_saving() {
        let mut documents = open_with_list();
        documents.add_emitter("effects/n.emitter.ron", default()).unwrap();
        assert!(documents.add_emitter("effects/n.emitter.ron", default()).is_err());
        assert!(documents.dirty_files(None).is_empty());
        let mut file = documents.scene(SCENE).unwrap().clone();
        file.emitters[0].source = EmitterSource::File("n.emitter.ron".into());
        documents.edit_scene(SCENE, file);
        assert_eq!(documents.dirty_files(Some(SCENE)), [SCENE, "effects/n.emitter.ron"]);
        // Changes to existing ones do, even if nothing uses them any more.
        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        assert!(documents.dirty_files(None).contains(&EMITTER.to_string()));
    }

    #[test]
    fn line_endings_are_kept() {
        let mut documents = open();
        documents.file_loaded(EMITTER, "(\r\nemitter: (rate: 1.0),\r\n)".into()).unwrap();
        let text = documents.to_save(EMITTER).unwrap();
        assert!(text.ends_with(")\r\n"), "{text:?}");
        assert!(!documents.to_save(SCENE).unwrap().contains('\r'));
    }

    #[test]
    fn comments_are_found_outside_strings() {
        assert!(has_comments("( // a\n)") && has_comments("(/* a */)"));
        assert!(!has_comments(r#"(path: "http://a\"//b")"#));
        assert!(has_comments(r#"(path: "a") // "b""#));
    }
}
