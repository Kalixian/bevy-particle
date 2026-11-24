//! Keyframes for emitters: spawn settings ([`EmitterTracks`]) and placement ([`EmitterMotion`]).
//!
//! Particles take these values when they spawn, at age 0, so the only clock that makes sense is
//! `Clock::Time`: seconds since the emitter started. Tracks with another clock, or without keys,
//! are ignored (effect files with another clock fail to load).

use bevy::{math::Vec4Swizzles, prelude::*};
use serde::{Deserialize, Serialize};

use crate::{
    area::SpawnArea,
    emitter::{EmitterState, ParticleEmitter},
    spread::Spread,
    track::{Keyable, Track},
    util::is_default,
};

/// Tracked lifetimes are at least this. A particle that spawned with lifetime 0 would already be
/// dead, and its slot would never be freed.
const MIN_LIFETIME: f32 = 1e-3;

pub(crate) struct EmitterTracksPlugin;

impl Plugin for EmitterTracksPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, move_emitters.before(TransformSystems::Propagate));
    }
}

/// Keyframed spawn settings, next to a [`ParticleEmitter`]. Each track replaces the emitter's
/// setting of that name while it's there.
///
/// Ranges are keyed as (from, to); the shape of their distribution stays the emitter's. Changes
/// only affect new particles, except for `mass_per_radius`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmitterTracks {
    #[serde(skip_serializing_if = "is_default")]
    pub rate: Option<Track<f32>>,
    /// Ignored if the emitter's particles are immortal.
    #[serde(skip_serializing_if = "is_default")]
    pub lifetime: Option<Track<Vec2>>,
    /// Ignored if it's for another kind of area than the emitter's.
    #[serde(skip_serializing_if = "is_default")]
    pub spawn: Option<AreaTracks>,
    #[serde(skip_serializing_if = "is_default")]
    pub speed: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub angle: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub radius: Option<Track<Vec2>>,
    /// Also changes the mass of particles that are already there.
    #[serde(skip_serializing_if = "is_default")]
    pub mass_per_radius: Option<Track<f32>>,
    #[serde(skip_serializing_if = "is_default")]
    pub red: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub green: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub blue: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub alpha: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub rotation: Option<Track<Vec2>>,
    #[serde(skip_serializing_if = "is_default")]
    pub spin: Option<Track<Vec2>>,
}

/// Keyframed dimensions of the [`SpawnArea`] of the same name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum AreaTracks {
    Rect {
        half_extents: Track<Vec2>,
    },
    Ellipse {
        radii: Track<Vec2>,
    },
    /// Both are (from, to).
    Ring {
        #[serde(default, skip_serializing_if = "is_default")]
        radius: Option<Track<Vec2>>,
        #[serde(default, skip_serializing_if = "is_default")]
        arc: Option<Track<Vec2>>,
    },
    Line {
        #[serde(default, skip_serializing_if = "is_default")]
        from: Option<Track<Vec2>>,
        #[serde(default, skip_serializing_if = "is_default")]
        to: Option<Track<Vec2>>,
    },
}

/// Keyframed placement of an emitter. Each track overwrites that part of its `Transform` every
/// frame; parts without one are left alone.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmitterMotion {
    /// Relative to the parent. Leaves z alone.
    #[serde(skip_serializing_if = "is_default")]
    pub position: Option<Track<Vec2>>,
    /// Radians, counter-clockwise.
    #[serde(skip_serializing_if = "is_default")]
    pub rotation: Option<Track<f32>>,
}

/// The value of an optional track, if it has any at `age`.
fn sample<T: Keyable>(track: &Option<Track<T>>, age: f32) -> Option<Vec4> {
    track.as_ref()?.sample_time(age)
}

fn set_number(value: &mut f32, track: &Option<Track<f32>>, age: f32) {
    if let Some(sampled) = sample(track, age) {
        *value = sampled.x;
    }
}

fn set_vec2(value: &mut Vec2, track: &Option<Track<Vec2>>, age: f32) {
    if let Some(sampled) = sample(track, age) {
        *value = sampled.xy();
    }
}

fn set_spread(spread: &mut Spread, track: &Option<Track<Vec2>>, age: f32) {
    if let Some(sampled) = sample(track, age) {
        spread.range = sampled.x..sampled.y;
    }
}

impl EmitterTracks {
    /// `emitter` as it is `age` seconds after it started.
    pub fn apply(&self, emitter: &ParticleEmitter, age: f32) -> ParticleEmitter {
        let mut result = emitter.clone();
        set_number(&mut result.rate, &self.rate, age);
        if let (Some(lifetime), Some(sampled)) =
            (&mut result.lifetime, sample(&self.lifetime, age))
        {
            lifetime.range = sampled.x.max(MIN_LIFETIME)..sampled.y.max(MIN_LIFETIME);
        }
        if let Some(tracks) = &self.spawn {
            tracks.apply(&mut result.spawn, age);
        }
        set_number(&mut result.mass_per_radius, &self.mass_per_radius, age);
        for (spread, track) in [
            (&mut result.speed, &self.speed),
            (&mut result.angle, &self.angle),
            (&mut result.radius, &self.radius),
            (&mut result.red, &self.red),
            (&mut result.green, &self.green),
            (&mut result.blue, &self.blue),
            (&mut result.alpha, &self.alpha),
            (&mut result.rotation, &self.rotation),
            (&mut result.spin, &self.spin),
        ] {
            set_spread(spread, track, age);
        }
        result
    }

    /// Why some of these tracks would be ignored on `emitter`, if any would.
    pub fn check(&self, emitter: &ParticleEmitter) -> Result<(), String> {
        check_clock("rate", &self.rate)?;
        check_clock("lifetime", &self.lifetime)?;
        check_clock("speed", &self.speed)?;
        check_clock("angle", &self.angle)?;
        check_clock("radius", &self.radius)?;
        check_clock("mass_per_radius", &self.mass_per_radius)?;
        check_clock("red", &self.red)?;
        check_clock("green", &self.green)?;
        check_clock("blue", &self.blue)?;
        check_clock("alpha", &self.alpha)?;
        check_clock("rotation", &self.rotation)?;
        check_clock("spin", &self.spin)?;
        if self.lifetime.is_some() && emitter.lifetime.is_none() {
            return Err("there's a lifetime track, but the particles are immortal".into());
        }
        if let Some(tracks) = &self.spawn {
            tracks.check(&emitter.spawn)?;
        }
        Ok(())
    }
}

impl AreaTracks {
    fn apply(&self, area: &mut SpawnArea, age: f32) {
        match (self, area) {
            (Self::Rect { half_extents: track }, SpawnArea::Rect { half_extents, .. }) => {
                if let Some(sampled) = track.sample_time(age) {
                    *half_extents = sampled.xy();
                }
            }
            (Self::Ellipse { radii: track }, SpawnArea::Ellipse { radii, .. }) => {
                if let Some(sampled) = track.sample_time(age) {
                    *radii = sampled.xy();
                }
            }
            (Self::Ring { radius, arc }, SpawnArea::Ring { radius: to_radius, arc: to_arc }) => {
                set_spread(to_radius, radius, age);
                set_spread(to_arc, arc, age);
            }
            (Self::Line { from, to }, SpawnArea::Line { from: to_from, to: to_to }) => {
                set_vec2(to_from, from, age);
                set_vec2(to_to, to, age);
            }
            _ => {}
        }
    }

    fn check(&self, area: &SpawnArea) -> Result<(), String> {
        match (self, area) {
            (Self::Rect { half_extents: track }, SpawnArea::Rect { .. })
            | (Self::Ellipse { radii: track }, SpawnArea::Ellipse { .. }) => {
                check_time("spawn", track)
            }
            (Self::Ring { radius: a, arc: b }, SpawnArea::Ring { .. })
            | (Self::Line { from: a, to: b }, SpawnArea::Line { .. }) => {
                check_clock("spawn", a)?;
                check_clock("spawn", b)
            }
            _ => Err("the spawn tracks are for another kind of area than the emitter's".into()),
        }
    }
}

impl EmitterMotion {
    pub fn is_empty(&self) -> bool {
        self.position.is_none() && self.rotation.is_none()
    }

    /// Why some of these tracks would be ignored, if any would.
    pub fn check(&self) -> Result<(), String> {
        check_clock("position", &self.position)?;
        check_clock("rotation", &self.rotation)
    }
}

fn check_clock<T>(name: &str, track: &Option<Track<T>>) -> Result<(), String> {
    track.as_ref().map_or(Ok(()), |track| check_time(name, track))
}

fn check_time<T>(name: &str, track: &Track<T>) -> Result<(), String> {
    if track.is_over_time() {
        Ok(())
    } else {
        Err(format!("the {name} track of an emitter needs `clock: Time`"))
    }
}

fn move_emitters(mut emitters: Query<(&EmitterMotion, &EmitterState, &mut Transform)>) {
    for (motion, state, mut transform) in &mut emitters {
        if let Some(position) = sample(&motion.position, state.age()) {
            let position = position.xy().extend(transform.translation.z);
            if transform.translation != position {
                transform.translation = position;
            }
        }
        if let Some(rotation) = sample(&motion.rotation, state.age()) {
            let rotation = Quat::from_rotation_z(rotation.x);
            if transform.rotation != rotation {
                transform.rotation = rotation;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spread::Shape;

    /// Goes from (0, 10) to (10, 20) in two seconds.
    fn pair() -> Option<Track<Vec2>> {
        Some(Track::over_time([(0.0, Vec2::new(0.0, 10.0)), (2.0, Vec2::new(10.0, 20.0))]))
    }

    #[test]
    fn tracks_replace_values_and_keep_shapes() {
        let emitter = ParticleEmitter {
            rate: 7.0,
            speed: Spread::new(1.0..2.0, Shape::Low(3.0)),
            spin: (5.0..6.0).into(),
            ..default()
        };
        let tracks = EmitterTracks {
            rate: Some(Track::over_time([(0.0, 0.0), (1.0, 100.0)])),
            speed: pair(),
            ..default()
        };
        let result = tracks.apply(&emitter, 1.0);
        assert_eq!((result.rate, &result.speed.range), (100.0, &(5.0..15.0)));
        assert!(matches!(result.speed.shape, Shape::Low(3.0)));
        assert_eq!(result.spin.range, 5.0..6.0);
    }

    #[test]
    fn every_range_has_its_own_track() {
        let names = ["speed", "angle", "radius", "red", "green", "blue", "alpha", "rotation", "spin"];
        for name in names {
            let text = format!(
                "#![enable(implicit_some)]\n({name}: (clock: Time, keys: [(time: 0.0, value: (40.0, 41.0))]))"
            );
            let tracks: EmitterTracks = ron::from_str(&text).unwrap();
            let before = ParticleEmitter::default();
            let after = tracks.apply(&before, 0.0);
            let ranges = |e: &ParticleEmitter| {
                [&e.speed, &e.angle, &e.radius, &e.red, &e.green, &e.blue, &e.alpha, &e.rotation, &e.spin]
                    .map(|spread| spread.range.clone())
            };
            for ((other, before), after) in names.iter().zip(ranges(&before)).zip(ranges(&after)) {
                let expected = if *other == name { 40.0..41.0 } else { before };
                assert_eq!(after, expected, "{other} with a track for {name}");
            }
        }
    }

    #[test]
    fn lifetimes_stay_above_zero_and_immortals_stay_immortal() {
        let tracks = EmitterTracks {
            lifetime: Some(Track::over_time([(0.0, Vec2::new(-1.0, 0.0))])),
            ..default()
        };
        let lifetime = tracks.apply(&default(), 0.0).lifetime.unwrap().range;
        assert_eq!(lifetime, MIN_LIFETIME..MIN_LIFETIME);

        let immortal = ParticleEmitter { lifetime: None, ..default() };
        assert!(tracks.apply(&immortal, 0.0).lifetime.is_none());
        assert!(tracks.check(&immortal).is_err());
    }

    #[test]
    fn other_clocks_and_empty_tracks_are_ignored() {
        let emitter = ParticleEmitter { rate: 7.0, ..default() };
        let life = Track::over_life([(0.0, 1.0)]);
        let empty = Track::over_time(Vec::<(f32, f32)>::new());
        for rate in [life, empty] {
            let tracks = EmitterTracks { rate: Some(rate), ..default() };
            assert_eq!(tracks.apply(&emitter, 0.0).rate, 7.0);
        }
    }

    #[test]
    fn area_tracks_need_the_same_kind_of_area() {
        let ring = |radius: std::ops::Range<f32>| SpawnArea::Ring {
            radius: radius.into(),
            arc: (0.0..1.0).into(),
        };
        let tracks = AreaTracks::Ring { radius: pair(), arc: None };
        let mut area = ring(3.0..4.0);
        tracks.apply(&mut area, 2.0);
        let SpawnArea::Ring { radius, arc } = &area else { unreachable!() };
        assert_eq!((&radius.range, &arc.range), (&(10.0..20.0), &(0.0..1.0)));
        assert!(tracks.check(&area).is_ok());

        let mut line = SpawnArea::Line { from: Vec2::X, to: Vec2::Y };
        tracks.apply(&mut line, 2.0);
        assert!(matches!(line, SpawnArea::Line { from: Vec2::X, to: Vec2::Y }));
        assert!(tracks.check(&line).is_err());
    }

    #[test]
    fn motion_only_changes_what_it_tracks() {
        let mut app = App::new();
        app.add_systems(Update, move_emitters);
        let start = Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(Quat::from_rotation_z(0.5));
        let moved = EmitterMotion { position: pair(), rotation: None };
        let turned = EmitterMotion {
            position: None,
            rotation: Some(Track::over_time([(0.0, 1.0)])),
        };
        let spawn = |app: &mut App, motion| {
            app.world_mut().spawn((motion, start, ParticleEmitter::default())).id()
        };
        let (moved, turned) = (spawn(&mut app, moved), spawn(&mut app, turned));
        app.update();

        let moved = app.world().get::<Transform>(moved).unwrap();
        assert_eq!(moved.translation, Vec3::new(0.0, 10.0, 3.0));
        assert_eq!(moved.rotation, start.rotation);
        let turned = app.world().get::<Transform>(turned).unwrap();
        assert_eq!(turned.translation, start.translation);
        assert_eq!(turned.rotation, Quat::from_rotation_z(1.0));
    }
}
