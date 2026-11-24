use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType},
};
use serde::{Deserialize, Serialize};

use crate::track::{Animated, GpuTrackRecord, ParamEncoder, Rgb};

/// Shared modifier plumbing. Which particles a modifier affects depends on where the
/// [`Modifiers`] (or [`TargetAttractor`]) component sits:
/// - on a `ParticleSpace`: every particle in the space;
/// - on a `ParticleEmitter`: only that emitter's particles.
///
/// Each frame, space modifiers run first, then emitter modifiers, each list in order.
pub struct ModifierPlugin;

impl Plugin for ModifierPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModifierUpload>()
            .init_resource::<AttractorTarget>()
            .add_systems(
                PostUpdate,
                clear_modifier_upload.in_set(ModifierSystems::Clear),
            );
    }
}

/// Pulls particles toward [`AttractorTarget`] like a [`Modifier::PointAttractor`] there; does
/// nothing while there's no target. Put it on a `ParticleSpace` to pull every particle, or on a
/// `ParticleEmitter` to pull only its own.
///
/// It runs after the owner's [`Modifiers`] list and doesn't change it.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetAttractor {
    /// Acceleration = strength / distance², like `PointAttractor`. Negative repels.
    pub strength: Animated<f32>,
    /// Upper limit on the acceleration.
    pub max_accel: Animated<f32>,
}

/// Where every [`TargetAttractor`] pulls to, in world space, e.g. the cursor. Set it before
/// `PostUpdate`; `None` turns them off.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct AttractorTarget(pub Option<Vec2>);

/// Upload order: `Clear` runs before any system that calls [`ModifierUpload::push`].
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModifierSystems {
    Clear,
}

/// Behaviors applied to particles every frame. Add a variant here and a matching `case` in
/// `apply_force` or `apply_appearance` in `particle_sim.wesl`.
///
/// Every parameter is an [`Animated`]: a constant or a keyframe `Track`, via `.into()`.
///
/// Color, alpha, size and mass start from the particle's spawn values every frame, so
/// appearance modifiers combine in order without building up over time.
///
/// Positions (`offset`) are relative to the owning space or emitter, so attractors move
/// with their owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Modifier {
    /// Constant acceleration, e.g. wind or downward gravity.
    Acceleration(Animated<Vec2>),
    /// Inverse-square pull toward a point: `strength / distance²`, capped at `max_accel`.
    /// Negative strength repels. Nearby particles are usually at the cap, so to fade the pull
    /// smoothly animate `max_accel` along with `strength`.
    PointAttractor {
        offset: Animated<Vec2>,
        strength: Animated<f32>,
        max_accel: Animated<f32>,
    },
    /// Swirl around a point: tangential `strength / distance`, capped at `max_accel`.
    /// Positive is counter-clockwise.
    Vortex {
        offset: Animated<Vec2>,
        strength: Animated<f32>,
        max_accel: Animated<f32>,
    },
    /// Exponential velocity damping, as a fraction lost per second (roughly).
    Drag(Animated<f32>),
    /// Bounces particles off the edges of a rectangle centered on the owner.
    /// `restitution` is the fraction of speed kept on impact.
    Bounds {
        half_extents: Animated<Vec2>,
        restitution: Animated<f32>,
    },
    /// Linear RGB color; HDR values drive bloom. Leaves alpha alone.
    Color(Blend, Animated<Rgb>),
    /// Opacity.
    Alpha(Blend, Animated<f32>),
    /// Radius. Doesn't change mass.
    Size(Blend, Animated<f32>),
    Mass(Blend, Animated<f32>),
}

/// How an appearance modifier combines with the particle's current value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blend {
    /// Replace it.
    Set,
    /// Multiply it, keeping per-particle variation.
    Scale,
}

impl Blend {
    fn as_gpu(self) -> f32 {
        match self {
            Blend::Set => 0.0,
            Blend::Scale => 1.0,
        }
    }
}

/// The modifier list of a space or emitter.
#[derive(Component, Debug, Clone, Default, Deref, DerefMut, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(pub Vec<Modifier>);

/// One modifier as seen by the GPU. Must match `struct Modifier` in `particle_sim.wesl`.
/// The meaning of `a` and `b` depends on `kind`; see [`GpuModifier::encode`].
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuModifier {
    pub kind: u32,
    /// This modifier's tracks, back to back in the track array.
    pub track_offset: u32,
    pub track_count: u32,
    pub a: Vec4,
    pub b: Vec4,
}

impl GpuModifier {
    /// `origin` is the owner's world position; offsets are resolved against it here.
    /// Per-particle tracks are appended to `records`. `None` if a track has no keys.
    fn encode(
        modifier: &Modifier,
        origin: Vec2,
        owner_age: f32,
        records: &mut Vec<GpuTrackRecord>,
    ) -> Option<Self> {
        let start = records.len();
        let origin = origin.extend(0.0).extend(0.0);
        let mut params = ParamEncoder::new(records, owner_age);
        // Slots 0-3 are `a`, 4-7 are `b`.
        let kind = match modifier {
            Modifier::Acceleration(accel) => {
                params.put(0, accel);
                0
            }
            Modifier::PointAttractor { offset, strength, max_accel } => {
                params.put_offset(0, offset, origin);
                params.put(2, strength);
                params.put(3, max_accel);
                1
            }
            Modifier::Vortex { offset, strength, max_accel } => {
                params.put_offset(0, offset, origin);
                params.put(2, strength);
                params.put(3, max_accel);
                2
            }
            Modifier::Drag(k) => {
                params.put(0, k);
                3
            }
            Modifier::Bounds { half_extents, restitution } => {
                params.put_raw(0, origin.x);
                params.put_raw(1, origin.y);
                params.put(2, half_extents);
                params.put(4, restitution);
                4
            }
            Modifier::Color(blend, color) => {
                params.put(0, color);
                params.put_raw(4, blend.as_gpu());
                5
            }
            Modifier::Alpha(blend, value) => {
                params.put(0, value);
                params.put_raw(4, blend.as_gpu());
                6
            }
            Modifier::Size(blend, value) => {
                params.put(0, value);
                params.put_raw(4, blend.as_gpu());
                7
            }
            Modifier::Mass(blend, value) => {
                params.put(0, value);
                params.put_raw(4, blend.as_gpu());
                8
            }
        };
        let Some((a, b, track_count)) = params.finish() else {
            records.truncate(start);
            return None;
        };
        Some(Self { kind, track_offset: start as u32, track_count, a, b })
    }
}

/// Every space and emitter modifier for this frame, in one array. Owners refer to their
/// slice of it by `(offset, count)`.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct ModifierUpload {
    modifiers: Vec<GpuModifier>,
    /// Keyframe tracks the modifiers refer to.
    tracks: Vec<GpuTrackRecord>,
    /// This frame's [`AttractorTarget`], copied in when the upload is cleared.
    target: Option<Vec2>,
}

impl ModifierUpload {
    /// Appends an owner's `modifiers`, then its [`TargetAttractor`] if there is a target, and
    /// returns their `(offset, count)` in the array. `origin` is the owner's world position and
    /// `owner_age` the seconds since it started.
    pub fn push(
        &mut self,
        modifiers: Option<&Modifiers>,
        attractor: Option<&TargetAttractor>,
        origin: Vec2,
        owner_age: f32,
    ) -> (u32, u32) {
        let offset = self.modifiers.len() as u32;
        if let Some(modifiers) = modifiers {
            for modifier in modifiers.iter() {
                self.push_one(modifier, origin, owner_age);
            }
        }
        if let (Some(attractor), Some(target)) = (attractor, self.target) {
            let modifier = Modifier::PointAttractor {
                offset: (target - origin).into(),
                strength: attractor.strength.clone(),
                max_accel: attractor.max_accel.clone(),
            };
            self.push_one(&modifier, origin, owner_age);
        }
        (offset, self.modifiers.len() as u32 - offset)
    }

    fn push_one(&mut self, modifier: &Modifier, origin: Vec2, owner_age: f32) {
        let encoded = GpuModifier::encode(modifier, origin, owner_age, &mut self.tracks);
        self.modifiers.extend(encoded);
    }

    /// The track array to upload. Never empty, like [`Self::gpu_data`].
    pub fn tracks_gpu_data(&self) -> Vec<GpuTrackRecord> {
        if self.tracks.is_empty() {
            vec![GpuTrackRecord::default()]
        } else {
            self.tracks.clone()
        }
    }

    /// The array to upload. Never empty, because a zero-sized storage buffer can't be bound.
    pub fn gpu_data(&self) -> Vec<GpuModifier> {
        if self.modifiers.is_empty() {
            vec![GpuModifier::default()]
        } else {
            self.modifiers.clone()
        }
    }
}

fn clear_modifier_upload(mut upload: ResMut<ModifierUpload>, target: Res<AttractorTarget>) {
    upload.modifiers.clear();
    upload.tracks.clear();
    upload.target = target.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::Track;

    fn encode(modifier: Modifier, age: f32) -> (Option<GpuModifier>, Vec<GpuTrackRecord>) {
        let mut records = Vec::new();
        let encoded = GpuModifier::encode(&modifier, Vec2::new(100.0, 10.0), age, &mut records);
        (encoded, records)
    }

    #[test]
    fn tracks_target_their_parameter_and_constants_stay_inline() {
        let (m, records) = encode(
            Modifier::PointAttractor {
                offset: Track::over_life([(0.0, Vec2::ZERO), (1.0, Vec2::X)]).into(),
                strength: 5.0.into(),
                max_accel: Track::over_seconds([(0.0, 1.0), (2.0, 3.0)]).looped().into(),
            },
            0.0,
        );
        let m = m.unwrap();
        assert_eq!((m.track_count, m.a.z, records.len()), (2, 5.0, 6));
        assert_eq!(records[0].info, UVec4::new(2, 0, 0, 2));
        // Keys are moved by the owner's position.
        assert_eq!(records[2].data.truncate().truncate(), Vec2::new(101.0, 10.0));
        // Seconds clock (1) + 2 · loop (1).
        assert_eq!(records[3].info, UVec4::new(2, 3, 3, 1));
        assert_eq!(records[3].data.x, 2.0);
    }

    #[test]
    fn over_time_tracks_become_constants() {
        let drag = Track::over_time([(0.0, 0.0), (2.0, 4.0)]);
        let (m, records) = encode(Modifier::Drag(drag.into()), 1.0);
        let m = m.unwrap();
        assert_eq!((m.a.x, m.track_count, records.len()), (2.0, 0, 0));
    }

    #[test]
    fn modifiers_with_an_empty_track_are_dropped() {
        let (m, records) = encode(
            Modifier::Bounds {
                half_extents: Track::over_life([(0.0, Vec2::ONE)]).into(),
                restitution: Track::over_life(Vec::<(f32, f32)>::new()).into(),
            },
            0.0,
        );
        assert!(m.is_none() && records.is_empty());
    }
}
