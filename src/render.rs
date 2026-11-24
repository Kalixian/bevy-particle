use bevy::{prelude::*, render::render_resource::ShaderType};
use serde::{Deserialize, Serialize};

/// How a space's particles are drawn. Put it on the [`ParticleSpace`](crate::ParticleSpace);
/// without it, particles are hard-edged, alpha-blended disks.
///
/// Spaces are sorted among sprites and other transparent 2D items by their `Transform`'s z, and
/// respect `Visibility` (hidden spaces are still simulated) and `RenderLayers`.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct ParticleRender {
    pub blend: BlendMode,
    /// How much of a disk's radius fades out, from 0 (hard edge) to 1 (fades from the center).
    /// Textured particles ignore it.
    pub softness: f32,
    /// Stretches particles along their velocity, by the distance they travel in this many
    /// seconds. Above 0, textures are turned so that +X points along the velocity.
    pub stretch: f32,
    /// Drawn on a square around the particle, tinted by its color. Nothing is drawn until it
    /// has loaded.
    pub texture: Option<ParticleTexture>,
}

/// How particles combine with what's behind them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    /// Covers what's behind, according to alpha.
    #[default]
    Alpha,
    /// Adds light; alpha scales how much. Overlapping particles get brighter, in any order.
    Additive,
}

/// An image, optionally a sheet of animation frames in rows from the top left.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleTexture {
    pub image: Handle<Image>,
    pub columns: u32,
    pub rows: u32,
    pub playback: Playback,
}

impl ParticleTexture {
    /// A single-frame texture.
    pub fn new(image: Handle<Image>) -> Self {
        Self { image, columns: 1, rows: 1, playback: Playback::default() }
    }
}

/// Which frame of a [`ParticleTexture`] each particle shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum Playback {
    /// Plays through once over the particle's life. Immortal particles stay on the first frame.
    #[default]
    OverLife,
    /// Loops at this many frames per second, from when the particle spawned.
    Fps(f32),
    /// One random frame per particle.
    Random,
}

/// Per-space draw settings. Must match `struct RenderParams` in `particle_draw.wesl`.
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub(crate) struct GpuRenderParams {
    pub softness: f32,
    pub stretch: f32,
    pub z: f32,
    /// 1 to draw the texture, 0 for disks.
    pub textured: u32,
    pub columns: u32,
    pub rows: u32,
    /// 0 = over life, 1 = fps, 2 = random.
    pub playback: u32,
    pub fps: f32,
}

impl ParticleRender {
    pub(crate) fn to_gpu(&self, z: f32) -> GpuRenderParams {
        let mut params = GpuRenderParams {
            softness: self.softness,
            stretch: self.stretch.max(0.0),
            z,
            textured: 0,
            columns: 1,
            rows: 1,
            playback: 0,
            fps: 0.0,
        };
        if let Some(texture) = &self.texture {
            params.textured = 1;
            params.columns = texture.columns.max(1);
            params.rows = texture.rows.max(1);
            (params.playback, params.fps) = match texture.playback {
                Playback::OverLife => (0, 0.0),
                Playback::Fps(fps) => (1, fps),
                Playback::Random => (2, 0.0),
            };
        }
        params
    }
}
