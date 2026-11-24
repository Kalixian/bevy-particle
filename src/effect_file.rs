//! Effects as they're written in `.effect.ron` and `.emitter.ron` files.
//!
//! An [`EffectFile`] keeps what a [`ParticleEffect`] doesn't: which emitters come from files of
//! their own, and where textures are. That makes it the form to edit and save;
//! [`EffectFile::build`] turns it into an effect that can be spawned.

use bevy::{
    asset::{AssetPath, ParseAssetPathError},
    prelude::*,
};
use ron::{extensions::Extensions, ser::PrettyConfig};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    effect::{EffectEmitter, EmitterDef, ParticleEffect},
    emitter_tracks::EmitterMotion,
    gravity::Gravity,
    modifier::{Modifiers, TargetAttractor},
    render::{BlendMode, ParticleRender, ParticleTexture, Playback},
    space::ParticleSpace,
    util::is_default,
};

/// The contents of an `.effect.ron` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectFile {
    pub space: ParticleSpace,
    #[serde(default, skip_serializing_if = "is_default")]
    pub render: RenderFile,
    #[serde(default, skip_serializing_if = "is_default")]
    pub gravity: Option<Gravity>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub attractor: Option<TargetAttractor>,
    /// Applied to every particle in the space.
    #[serde(default, skip_serializing_if = "is_default")]
    pub modifiers: Modifiers,
    pub emitters: Vec<EmitterEntry>,
}

/// [`ParticleRender`] as written in a file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderFile {
    #[serde(skip_serializing_if = "is_default")]
    pub blend: BlendMode,
    #[serde(skip_serializing_if = "is_default")]
    pub softness: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub stretch: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub texture: Option<TextureFile>,
}

/// [`ParticleTexture`] as written in a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureFile {
    /// Path of an image, found like [`EmitterSource::File`].
    pub path: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub columns: u32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub rows: u32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub playback: Playback,
}

fn one() -> u32 {
    1
}

fn is_one(value: &u32) -> bool {
    *value == 1
}

/// An emitter placed in an effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitterEntry {
    /// Relative to the space.
    #[serde(default, skip_serializing_if = "is_default")]
    pub position: Vec2,
    /// Radians, counter-clockwise.
    #[serde(default, skip_serializing_if = "is_default")]
    pub rotation: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub motion: EmitterMotion,
    pub source: EmitterSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EmitterSource {
    /// Path of an emitter file, relative to the effect file (or to the asset folder, with a
    /// leading `/`).
    File(String),
    Inline(Box<EmitterDef>),
}

impl EffectFile {
    pub fn from_ron(text: &str) -> Result<Self, BevyError> {
        parse(text)
    }

    pub fn to_ron(&self) -> Result<String, BevyError> {
        to_ron(self)
    }

    /// The emitter files of the effect at `path`, in order, possibly with duplicates.
    pub fn emitter_paths(
        &self,
        path: &AssetPath,
    ) -> Result<Vec<AssetPath<'static>>, ParseAssetPathError> {
        self.emitters
            .iter()
            .filter_map(|entry| match &entry.source {
                EmitterSource::File(file) => Some(path.resolve_embed_str(file)),
                EmitterSource::Inline(_) => None,
            })
            .collect()
    }

    /// The effect, when its file is at `path`. `emitter` and `image` get what's at a path.
    pub fn build(
        &self,
        path: &AssetPath,
        mut emitter: impl FnMut(&AssetPath<'static>) -> Result<EmitterDef, BevyError>,
        mut image: impl FnMut(AssetPath<'static>) -> Handle<Image>,
    ) -> Result<ParticleEffect, BevyError> {
        let mut emitters = Vec::with_capacity(self.emitters.len());
        for EmitterEntry { position, rotation, motion, source } in &self.emitters {
            motion.check()?;
            let def = match source {
                EmitterSource::Inline(def) => (**def).clone(),
                EmitterSource::File(file) => emitter(&path.resolve_embed_str(file)?)?,
            };
            if let Some(tracks) = &def.tracks {
                tracks.check(&def.emitter)?;
            }
            emitters.push(EffectEmitter {
                position: *position,
                rotation: *rotation,
                motion: motion.clone(),
                def,
            });
        }
        let texture = match &self.render.texture {
            Some(TextureFile { path: file, columns, rows, playback }) => Some(ParticleTexture {
                image: image(path.resolve_embed_str(file)?),
                columns: *columns,
                rows: *rows,
                playback: *playback,
            }),
            None => None,
        };
        Ok(ParticleEffect {
            space: self.space.clone(),
            render: ParticleRender {
                blend: self.render.blend,
                softness: self.render.softness,
                stretch: self.render.stretch,
                texture,
            },
            gravity: self.gravity,
            attractor: self.attractor.clone(),
            modifiers: self.modifiers.clone(),
            emitters,
        })
    }
}

impl EmitterDef {
    /// Reads the contents of an `.emitter.ron` file.
    pub fn from_ron(text: &str) -> Result<Self, BevyError> {
        parse(text)
    }

    pub fn to_ron(&self) -> Result<String, BevyError> {
        to_ron(self)
    }
}

/// Lets files say `gravity: (..)` instead of `gravity: Some((..))`.
fn options() -> ron::Options {
    ron::Options::default().with_default_extension(Extensions::IMPLICIT_SOME)
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T, BevyError> {
    Ok(options().from_str(text)?)
}

fn to_ron<T: Serialize>(value: &T) -> Result<String, BevyError> {
    let config = PrettyConfig::new().new_line("
");
    Ok(reflow(&options().to_string_pretty(value, config)?))
}

/// Longest line that [`reflow`] makes.
const MAX_WIDTH: usize = 100;

/// Puts each bracketed group of pretty-printed RON on a single line if it fits.
fn reflow(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    // Where the groups that haven't ended yet start in `lines`.
    let mut open: Vec<usize> = Vec::new();
    for line in text.lines() {
        let content = line.trim_start();
        // Pretty-printing only ends lines with an opening bracket, or starts them with a closing
        // one, where a group is split over lines. (Line breaks in strings are escaped.)
        if content.starts_with([')', ']', '}']) && let Some(start) = open.pop() {
            let joined = join_group(&lines[start], &lines[start + 1..], content);
            if joined.len() <= MAX_WIDTH && !lines[start + 1..].iter().any(|l| opens_group(l)) {
                lines.truncate(start);
                lines.push(joined);
                continue;
            }
        }
        if opens_group(line) {
            open.push(lines.len());
        }
        lines.push(line.to_string());
    }
    lines.iter().flat_map(|line| [line.as_str(), "
"]).collect()
}

fn opens_group(line: &str) -> bool {
    line.ends_with(['(', '[', '{'])
}

fn join_group(first: &str, items: &[String], last: &str) -> String {
    let items: Vec<_> = items.iter().map(|item| item.trim_start()).collect();
    let items = items.join(" ");
    format!("{first}{}{last}", items.trim_end_matches(','))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// (name, text) of every shipped file with this extension.
    fn shipped(extension: &str) -> Vec<(String, String)> {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/effects");
        let files: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_name().to_str().unwrap().ends_with(extension))
            .map(|entry| {
                let name = entry.file_name().into_string().unwrap();
                (name, fs::read_to_string(entry.path()).unwrap())
            })
            .collect();
        assert!(!files.is_empty());
        files
    }

    #[test]
    fn shipped_effects_survive_saving() {
        for (name, text) in shipped(".effect.ron") {
            let file = EffectFile::from_ron(&text).unwrap();
            let saved = file.to_ron().unwrap();
            assert_eq!(EffectFile::from_ron(&saved).unwrap(), file, "{name}:\n{saved}");
            // Saving again changes nothing.
            assert_eq!(EffectFile::from_ron(&saved).unwrap().to_ron().unwrap(), saved, "{name}");
        }
    }

    #[test]
    fn shipped_emitters_survive_saving() {
        for (name, text) in shipped(".emitter.ron") {
            let def = EmitterDef::from_ron(&text).unwrap();
            let saved = def.to_ron().unwrap();
            assert_eq!(EmitterDef::from_ron(&saved).unwrap(), def, "{name}:\n{saved}");
        }
    }

    #[test]
    fn defaults_are_left_out_but_other_values_are_kept() {
        let file = EffectFile::from_ron(
            "(space: (capacity: 8), emitters: [(source: Inline((emitter: (lifetime: None, rate: 3.0))))])",
        )
        .unwrap();
        let saved = file.to_ron().unwrap();
        assert_eq!(
            saved,
            "(space: (capacity: 8), emitters: [(source: Inline((emitter: (rate: 3.0, lifetime: None))))])
"
        );
        assert_eq!(EffectFile::from_ron(&saved).unwrap(), file);
    }

    #[test]
    fn strings_dont_confuse_the_layout() {
        for path in ["a(", "b)
)", "c,", "d\"[", "(
("] {
            let file = EffectFile {
                render: RenderFile {
                    texture: Some(TextureFile {
                        path: path.repeat(30),
                        columns: 2,
                        rows: 1,
                        playback: default(),
                    }),
                    ..default()
                },
                ..EffectFile::from_ron("(space: (capacity: 8), emitters: [])").unwrap()
            };
            let saved = file.to_ron().unwrap();
            assert_eq!(EffectFile::from_ron(&saved).unwrap(), file, "{saved}");
        }
    }

    #[test]
    fn custom_shapes_cant_be_saved() {
        let mut def = EmitterDef::default();
        def.emitter.speed.shape = crate::Shape::Custom(|u| u);
        assert!(def.to_ron().is_err());
    }
}
