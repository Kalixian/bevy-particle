use bevy::prelude::*;

/// Input that the UI is using, which the scene's controls should ignore. Nothing is captured
/// without the editor.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct InputCapture {
    pub pointer: bool,
    pub keyboard: bool,
}

/// Updates [`InputCapture`]. Scene controls run after this set.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaptureSystems;
