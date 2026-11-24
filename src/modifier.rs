use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType},
};
use serde::{Deserialize, Serialize};

pub use crate::modifier_kinds::Phase;

use crate::{
    modifier_kinds::ModifierKind,
    track::{Animated, GpuTrackRecord, ParamEncoder, Rgb},
};

/// Shared modifier plumbing. Which particles a modifier affects depends on where the
/// [`Modifiers`] (or [`TargetAttractor`]) component sits:
/// - on a `ParticleSpace`: every particle in the space;
/// - on a `ParticleEmitter`: only that emitter's particles.
///
/// Each frame, all forces run first, then constraints ([`Modifier::Bounds`]), then appearance
/// modifiers. Within each of those, space modifiers run before emitter modifiers, each list in
/// order.
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
/// It runs after the forces in the owner's [`Modifiers`] and doesn't change that list.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// Behaviors applied to particles every frame.
///
/// They run in this order: all forces, on top of any [`Gravity`](crate::Gravity); then
/// constraints ([`Bounds`](Self::Bounds)), so no force can push particles past them; then
/// appearance.
///
/// Every parameter is an [`Animated`]: a constant or a keyframe `Track`, via `.into()`.
///
/// Color, alpha, size and mass start from the particle's spawn values every frame, so
/// appearance modifiers combine in order without building up over time.
///
/// Positions (`offset`) are relative to the owning space or emitter, so attractors move
/// with their owner.
// To add one: a variant here, its arms in `kind` and `encode`, its row in `modifier_kinds.rs`,
// and a `case` (and imports) in `particle_sim.wesl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl Modifier {
    fn kind(&self) -> ModifierKind {
        match self {
            Modifier::Acceleration(_) => ModifierKind::Acceleration,
            Modifier::PointAttractor { .. } => ModifierKind::PointAttractor,
            Modifier::Vortex { .. } => ModifierKind::Vortex,
            Modifier::Drag(_) => ModifierKind::Drag,
            Modifier::Bounds { .. } => ModifierKind::Bounds,
            Modifier::Color(..) => ModifierKind::Color,
            Modifier::Alpha(..) => ModifierKind::Alpha,
            Modifier::Size(..) => ModifierKind::Size,
            Modifier::Mass(..) => ModifierKind::Mass,
        }
    }

    /// When it runs, relative to other modifiers.
    pub fn phase(&self) -> Phase {
        self.kind().def().phase
    }
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
#[derive(Component, Debug, Clone, Default, Deref, DerefMut, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(pub Vec<Modifier>);

/// One modifier as seen by the GPU. Must match `struct Modifier` in `types.wesl`.
/// The meaning of `a` and `b` depends on `kind`; see `modifier_kinds.rs`.
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
        let kind = modifier.kind();
        let mut params = ParamEncoder::new(records, owner_age, kind.def().params);
        match modifier {
            Modifier::Acceleration(accel) => params.put(accel),
            Modifier::PointAttractor { offset, strength, max_accel }
            | Modifier::Vortex { offset, strength, max_accel } => {
                params.put_offset(offset, origin);
                params.put(strength);
                params.put(max_accel);
            }
            Modifier::Drag(k) => params.put(k),
            Modifier::Bounds { half_extents, restitution } => {
                params.put_raw(origin.truncate().truncate());
                params.put(half_extents);
                params.put(restitution);
            }
            Modifier::Color(blend, color) => {
                params.put(color);
                params.put_raw(blend.as_gpu());
            }
            Modifier::Alpha(blend, value)
            | Modifier::Size(blend, value)
            | Modifier::Mass(blend, value) => {
                params.put(value);
                params.put_raw(blend.as_gpu());
            }
        }
        let Some((a, b, track_count)) = params.finish() else {
            records.truncate(start);
            return None;
        };
        Some(Self { kind: kind.id(), track_offset: start as u32, track_count, a, b })
    }
}

/// Every space and emitter modifier for this frame, in one array. Owners refer to their
/// slice of it by the range that [`ModifierUpload::push`] returns.
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
    /// Appends an owner's `modifiers`, grouped by phase, with its [`TargetAttractor`] (if there
    /// is a target) after the forces. Returns where in the array its forces, constraints and
    /// appearance modifiers start, and where they end. `origin` is the owner's world position and
    /// `owner_age` the seconds since it started.
    pub fn push(
        &mut self,
        modifiers: Option<&Modifiers>,
        attractor: Option<&TargetAttractor>,
        origin: Vec2,
        owner_age: f32,
    ) -> UVec4 {
        let mut range = UVec4::ZERO;
        for (i, phase) in Phase::ALL.into_iter().enumerate() {
            range[i] = self.modifiers.len() as u32;
            for modifier in modifiers.iter().flat_map(|list| list.iter()) {
                if modifier.phase() == phase {
                    self.push_one(modifier, origin, owner_age);
                }
            }
            if let (Phase::Force, Some(attractor), Some(target)) = (phase, attractor, self.target) {
                let modifier = Modifier::PointAttractor {
                    offset: (target - origin).into(),
                    strength: attractor.strength.clone(),
                    max_accel: attractor.max_accel.clone(),
                };
                self.push_one(&modifier, origin, owner_age);
            }
        }
        range.w = self.modifiers.len() as u32;
        range
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
    fn modifiers_are_grouped_by_phase_with_the_target_after_forces() {
        let bounds =
            Modifier::Bounds { half_extents: Vec2::ONE.into(), restitution: 1.0.into() };
        let list = Modifiers(vec![
            Modifier::Alpha(Blend::Set, 0.5.into()),
            bounds,
            Modifier::Drag(1.0.into()),
            Modifier::Size(Blend::Scale, 2.0.into()),
            Modifier::Acceleration(Vec2::X.into()),
        ]);
        let attractor = TargetAttractor { strength: 1.0.into(), max_accel: 1.0.into() };
        let mut upload = ModifierUpload { target: Some(Vec2::ONE), ..default() };
        upload.push(Some(&Modifiers(vec![Modifier::Drag(0.0.into())])), None, Vec2::ZERO, 0.0);
        let range = upload.push(Some(&list), Some(&attractor), Vec2::ZERO, 0.0);

        assert_eq!(range, UVec4::new(1, 4, 5, 7));
        use ModifierKind::*;
        let kinds: Vec<_> = upload.modifiers[1..].iter().map(|m| m.kind).collect();
        let expected = [Drag, Acceleration, PointAttractor, Bounds, Alpha, Size];
        assert_eq!(kinds, expected.map(ModifierKind::id));
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
