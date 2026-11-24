//! GPU-simulated 2D particles for Bevy.
//!
//! Add [`ParticlePlugin`], then spawn a [`ParticleSpace`] with [`ParticleEmitter`]s as children,
//! or load a [`ParticleEffect`] from a file and [`spawn`](ParticleEffect::spawn) it.
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_particle::{Modifier, Modifiers, ParticleEmitter, ParticlePlugin, ParticleSpace};
//!
//! fn main() {
//!     App::new()
//!         .add_plugins((DefaultPlugins, ParticlePlugin))
//!         .add_systems(Startup, setup)
//!         .run();
//! }
//!
//! fn setup(mut commands: Commands) {
//!     commands.spawn(Camera2d);
//!     commands.spawn((
//!         ParticleSpace { capacity: 10_000 },
//!         Modifiers(vec![Modifier::Drag(0.5.into())]),
//!         children![ParticleEmitter { rate: 1000.0, ..default() }],
//!     ));
//! }
//! ```

mod effect;
mod emitter;
mod gpu;
mod gravity;
mod modifier;
mod modifier_kinds;
mod particle;
mod render;
mod space;
mod spread;
mod track;

use bevy::prelude::*;

pub use effect::{EffectEmitter, EmitterDef, ParticleEffect};
pub use emitter::ParticleEmitter;
pub use gravity::{Gravity, GravityMethod, MAX_GRAVITY_BODIES};
pub use modifier::{AttractorTarget, Blend, Modifier, Modifiers, TargetAttractor};
pub use render::{BlendMode, ParticleRender, ParticleTexture, Playback};
pub use space::ParticleSpace;
pub use spread::{Shape, Spread};
pub use track::{Animated, Clock, Ease, Key, Keyable, Repeat, Rgb, Track};

/// Everything needed to simulate and draw particles. Add it after `DefaultPlugins`.
pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            modifier::ModifierPlugin,
            space::SpacePlugin,
            emitter::EmitterPlugin,
            gpu::ParticleGpuPlugin,
            effect::EffectPlugin,
        ));
    }
}
