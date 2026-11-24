//! The files being edited, as they are in memory and as they were last saved.

use std::collections::{BTreeMap, BTreeSet};

use bevy::{asset::AssetPath, prelude::*};
use bevy_particle::{EffectFile, EmitterDef, ParticleEffect};

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

/// One file.
pub struct Document<T> {
    value: T,
    saved: T,
    /// The file's text, which `saved` is the meaning of.
    text: String,
    /// The file's new text, if it was changed by something else while there were unsaved changes.
    conflict: Option<String>,
}

impl<T: Contents> Document<T> {
    fn open(text: String) -> Result<Self, BevyError> {
        let value = T::parse(&text)?;
        Ok(Self { saved: value.clone(), value, text, conflict: None })
    }

    fn is_dirty(&self) -> bool {
        self.value != self.saved
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
        self.text = text;
        self.conflict = None;
    }
}

/// Calls a method of the document at a path, of either kind.
macro_rules! with_document {
    ($self:ident, $path:expr, $document:ident => $result:expr) => {
        if let Some($document) = $self.scenes.get_mut($path) {
            Some($result)
        } else if let Some($document) = $self.emitters.get_mut($path) {
            Some($result)
        } else {
            None
        }
    };
}

/// Every open file, by its path in the asset folder.
#[derive(Resource, Default)]
pub struct Documents {
    scenes: BTreeMap<String, Document<EffectFile>>,
    emitters: BTreeMap<String, Document<EmitterDef>>,
    /// Files whose value changed since [`Self::take_changed`].
    changed: BTreeSet<String>,
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
                self.changed.insert(path.into());
            }
            Some(result) => {
                result?;
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
        self.scenes.contains_key(path) || self.emitters.contains_key(path)
    }

    /// Replaces a file's value with that of `text`, as an unsaved change.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), expect(dead_code, reason = "web only"))]
    pub fn replace(&mut self, path: &str, text: &str) -> Result<(), BevyError> {
        if let Some(document) = self.scenes.get_mut(path) {
            let old = std::mem::replace(&mut document.value, EffectFile::from_ron(text)?);
            // Refuse what can't be shown.
            if let Err(error) = self.build(path, |_| Handle::default()) {
                if let Some(document) = self.scenes.get_mut(path) {
                    document.value = old;
                }
                return Err(error);
            }
        } else if let Some(document) = self.emitters.get_mut(path) {
            let def = EmitterDef::from_ron(text)?;
            if let Some(tracks) = &def.tracks {
                tracks.check(&def.emitter)?;
            }
            document.value = def;
        } else {
            return Err(format!("{path} isn't open").into());
        }
        self.changed.insert(path.into());
        Ok(())
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
    }

    /// The files with unsaved changes, out of those of a scene or out of all.
    pub fn dirty_files(&self, scene: Option<&str>) -> Vec<String> {
        let files = match scene {
            Some(scene) => self.files_of(scene),
            None => self.scenes.keys().chain(self.emitters.keys()).cloned().collect(),
        };
        files.into_iter().filter(|path| self.is_dirty(path)).collect()
    }

    /// The first file of a scene that was also changed by something else.
    pub fn conflict_in(&self, scene: &str) -> Option<String> {
        self.files_of(scene).into_iter().find(|path| {
            self.scenes.get(path).is_some_and(|document| document.conflict.is_some())
                || self.emitters.get(path).is_some_and(|document| document.conflict.is_some())
        })
    }

    pub fn resolve(&mut self, path: &str, keep_mine: bool) -> Result<(), BevyError> {
        self.changed.insert(path.into());
        with_document!(self, path, document => document.resolve(keep_mine)).unwrap_or(Ok(()))
    }

    /// Drops a file's unsaved changes.
    pub fn revert(&mut self, path: &str) {
        self.changed.insert(path.into());
        with_document!(self, path, document => document.value = document.saved.clone());
    }

    /// Whether saving would lose comments, which aren't part of a file's value.
    pub fn has_comments(&self, path: &str) -> bool {
        let scene = self.scenes.get(path).map(|document| &document.text);
        let text = scene.or(self.emitters.get(path).map(|document| &document.text));
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
        let document = self.scenes.get(scene).ok_or_else(|| format!("{scene} isn't open"))?;
        let emitter = |path: &AssetPath<'static>| {
            let path = document_path(path);
            match self.emitters.get(&path) {
                Some(document) => Ok(document.value.clone()),
                None => Err(format!("{path} isn't open").into()),
            }
        };
        document.value.build(&AssetPath::parse(scene), emitter, image)
    }
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
        assert!(documents.take_changed().is_empty() && documents.conflict_in(SCENE).is_none());
    }

    #[test]
    fn outside_changes_are_adopted_unless_there_are_unsaved_ones() {
        let mut documents = open();
        documents.file_loaded(EMITTER, emitter(2.0)).unwrap();
        assert_eq!((rate(&documents), documents.is_dirty(EMITTER)), (2.0, false));

        documents.replace(EMITTER, &emitter(5.0)).unwrap();
        documents.file_loaded(EMITTER, emitter(3.0)).unwrap();
        assert_eq!(rate(&documents), 5.0);
        assert_eq!(documents.conflict_in(SCENE).as_deref(), Some(EMITTER));

        documents.resolve(EMITTER, true).unwrap();
        assert!(documents.conflict_in(SCENE).is_none() && documents.is_dirty(EMITTER));
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
