//! Keyframe tracks: values that change over time, interpolated between keys.
//!
//! Any modifier parameter is an [`Animated`], either a constant or a [`Track`]. Emitters have
//! tracks too; see `emitter_tracks.rs`.

use bevy::{prelude::*, render::render_resource::ShaderType};
use serde::{Deserialize, Serialize};

use crate::{modifier_kinds::ParamDef, util::is_default};

/// A type that can be keyframed. Values are interpolated as (up to) four floats.
pub trait Keyable: Copy {
    /// How many components of [`Keyable::to_vec4`] are used.
    const WIDTH: u32;
    fn to_vec4(self) -> Vec4;
}

impl Keyable for f32 {
    const WIDTH: u32 = 1;
    fn to_vec4(self) -> Vec4 {
        Vec4::new(self, 0.0, 0.0, 0.0)
    }
}

impl Keyable for Vec2 {
    const WIDTH: u32 = 2;
    fn to_vec4(self) -> Vec4 {
        self.extend(0.0).extend(0.0)
    }
}

/// A linear RGB color. Values above 1.0 are HDR and drive bloom. Alpha is separate (see
/// `Modifier::Alpha`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Keyable for Rgb {
    const WIDTH: u32 = 3;
    fn to_vec4(self) -> Vec4 {
        Vec4::new(self.0, self.1, self.2, 0.0)
    }
}

/// How a key blends toward the next one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ease {
    #[default]
    Linear,
    /// Smoothstep: slow at both keys.
    Smooth,
    /// Hold this key's value until the next key.
    Step,
}

/// What a track's key times measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Clock {
    /// Fraction of the particle's life, 0 to 1.
    Life,
    /// Seconds since the particle spawned.
    Seconds,
    /// Seconds since the owning space or emitter started: the same for every particle.
    Time,
}

/// What a track does after its last key. Cycles run from time 0 to the last key's time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Once,
    Loop,
    PingPong,
}

/// Written as `(time, value)` or `(time, value, ease)` in code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key<T> {
    pub time: f32,
    pub value: T,
    #[serde(default, skip_serializing_if = "is_default")]
    pub ease: Ease,
}

impl<T> From<(f32, T)> for Key<T> {
    fn from((time, value): (f32, T)) -> Self {
        Self { time, value, ease: Ease::default() }
    }
}

impl<T> From<(f32, T, Ease)> for Key<T> {
    fn from((time, value, ease): (f32, T, Ease)) -> Self {
        Self { time, value, ease }
    }
}

/// Keys sorted by time. The first and last values hold outside the keyed range, and two
/// keys at the same time make a hard cut. A modifier with an empty track does nothing.
///
/// ```ignore
/// Track::over_life([(0.0, 0.0), (0.2, 1.0), (1.0, 0.0)])
/// Track::over_time([(0.0, 10.0), (1.0, 50.0)]).ping_pong().ease(Ease::Smooth)
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "TrackData<T>")]
pub struct Track<T> {
    pub clock: Clock,
    #[serde(default, skip_serializing_if = "is_default")]
    pub repeat: Repeat,
    keys: Vec<Key<T>>,
}

/// A [`Track`] as read from data files, with its keys not yet sorted.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackData<T> {
    keys: Vec<Key<T>>,
    clock: Clock,
    #[serde(default)]
    repeat: Repeat,
}

impl<T> From<TrackData<T>> for Track<T> {
    fn from(data: TrackData<T>) -> Self {
        let mut track = Self { keys: data.keys, clock: data.clock, repeat: data.repeat };
        track.sort_keys();
        track
    }
}

impl<T> Track<T> {
    /// Sorted by time.
    pub fn keys(&self) -> &[Key<T>] {
        &self.keys
    }

    /// Replaces the keys, which don't have to be sorted.
    pub fn set_keys(&mut self, keys: Vec<Key<T>>) {
        self.keys = keys;
        self.sort_keys();
    }

    /// Whether it's keyed on `Clock::Time`.
    pub(crate) fn is_over_time(&self) -> bool {
        self.clock == Clock::Time
    }

    /// Stable, so keys at the same time keep their written order.
    fn sort_keys(&mut self) {
        self.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    }
}

impl<T: Keyable> Track<T> {
    /// Key times are fractions of each particle's life, 0 to 1. On immortal particles, force
    /// parameters stay at their time-0 value and appearance modifiers are skipped.
    pub fn over_life<K: Into<Key<T>>>(keys: impl IntoIterator<Item = K>) -> Self {
        Self::new(Clock::Life, keys)
    }

    /// Key times are seconds since each particle spawned.
    pub fn over_seconds<K: Into<Key<T>>>(keys: impl IntoIterator<Item = K>) -> Self {
        Self::new(Clock::Seconds, keys)
    }

    /// Key times are seconds since the owning space or emitter started, so every particle
    /// sees the same value.
    pub fn over_time<K: Into<Key<T>>>(keys: impl IntoIterator<Item = K>) -> Self {
        Self::new(Clock::Time, keys)
    }

    fn new<K: Into<Key<T>>>(clock: Clock, keys: impl IntoIterator<Item = K>) -> Self {
        let keys = keys.into_iter().map(Into::into).collect();
        let mut track = Self { keys, clock, repeat: Repeat::Once };
        track.sort_keys();
        track
    }

    /// Starts over after the last key. No effect on `over_life` tracks.
    pub fn looped(mut self) -> Self {
        self.repeat = Repeat::Loop;
        self
    }

    /// Runs back and forth between time 0 and the last key. No effect on `over_life` tracks.
    pub fn ping_pong(mut self) -> Self {
        self.repeat = Repeat::PingPong;
        self
    }

    /// Sets the easing of every key.
    pub fn ease(mut self, ease: Ease) -> Self {
        for key in &mut self.keys {
            key.ease = ease;
        }
        self
    }

    fn repeat(&self) -> Repeat {
        if self.clock == Clock::Life { Repeat::Once } else { self.repeat }
    }

    /// The value when the owner is `age` seconds old, or `None` if there are no keys or they aren't
    /// on `Clock::Time`.
    pub(crate) fn sample_time(&self, age: f32) -> Option<Vec4> {
        if self.is_over_time() { self.sample(age) } else { None }
    }

    /// The value at clock time `t`, or `None` with no keys. Must match `sample_track` in
    /// `tracks.wesl`.
    fn sample(&self, t: f32) -> Option<Vec4> {
        let period = self.keys.last()?.time;
        let t = match self.repeat() {
            Repeat::Loop if period > 0.0 => t.rem_euclid(period),
            Repeat::PingPong if period > 0.0 => {
                period - (t.rem_euclid(2.0 * period) - period).abs()
            }
            _ => t,
        };
        let next = self.keys.partition_point(|key| key.time <= t);
        if next == 0 {
            return Some(self.keys[0].value.to_vec4());
        }
        let from = &self.keys[next - 1];
        let Some(to) = self.keys.get(next) else {
            return Some(from.value.to_vec4());
        };
        let x = (t - from.time) / (to.time - from.time);
        let x = match from.ease {
            Ease::Linear => x,
            Ease::Smooth => x * x * (3.0 - 2.0 * x),
            Ease::Step => 0.0,
        };
        Some(from.value.to_vec4().lerp(to.value.to_vec4(), x))
    }
}

/// A modifier parameter: a constant or a [`Track`]. Build either with `.into()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Animated<T> {
    Constant(T),
    Track(Track<T>),
}

impl<T> From<T> for Animated<T> {
    fn from(value: T) -> Self {
        Self::Constant(value)
    }
}

impl<T> From<Track<T>> for Animated<T> {
    fn from(track: Track<T>) -> Self {
        Self::Track(track)
    }
}

/// A track is one header record followed by one record per key. Must match
/// `struct TrackRecord` in `tracks.wesl`.
///
/// - Header: `data.x` = period, `info` = (key count, clock + 2 · repeat, slot, width).
/// - Key: `data` = value, `info` = (time as f32 bits, ease, 0, 0).
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuTrackRecord {
    pub data: Vec4,
    pub info: UVec4,
}

/// Collects one modifier's parameters, in the order its kind defines them, into the eight
/// floats of `GpuModifier`'s `a` and `b`.
pub(crate) struct ParamEncoder<'a> {
    records: &'a mut Vec<GpuTrackRecord>,
    owner_age: f32,
    /// The parameters not yet written.
    layout: std::slice::Iter<'static, ParamDef>,
    /// Where the next parameter goes.
    slot: usize,
    values: [f32; 8],
    track_count: u32,
    valid: bool,
}

impl<'a> ParamEncoder<'a> {
    /// `owner_age` is the current time of [`Track::over_time`] tracks.
    pub fn new(
        records: &'a mut Vec<GpuTrackRecord>,
        owner_age: f32,
        layout: &'static [ParamDef],
    ) -> Self {
        Self {
            records,
            owner_age,
            layout: layout.iter(),
            slot: 0,
            values: [0.0; 8],
            track_count: 0,
            valid: true,
        }
    }

    /// The slot of the next parameter, which must be `width` floats wide.
    fn next_slot(&mut self, width: usize) -> usize {
        let param = self.layout.next().expect("more parameters than the kind defines");
        assert_eq!(param.width, width, "parameter width differs from the kind's definition");
        let slot = self.slot;
        self.slot += width;
        slot
    }

    /// Writes the next parameter. Per-particle tracks are appended to the
    /// records for the GPU to resolve; everything else becomes a constant here.
    pub fn put<T: Keyable>(&mut self, param: &Animated<T>) {
        self.put_offset(param, Vec4::ZERO);
    }

    /// Like [`Self::put`], adding `offset` to the value (or to every key).
    pub fn put_offset<T: Keyable>(&mut self, param: &Animated<T>, offset: Vec4) {
        let slot = self.next_slot(T::WIDTH as usize);
        let value = match param {
            Animated::Constant(value) => Some(value.to_vec4()),
            Animated::Track(track) if track.clock == Clock::Time => track.sample(self.owner_age),
            Animated::Track(track) => {
                if let Some(last) = track.keys.last() {
                    let clock = (track.clock == Clock::Seconds) as u32;
                    self.records.push(GpuTrackRecord {
                        data: Vec4::new(last.time, 0.0, 0.0, 0.0),
                        info: UVec4::new(
                            track.keys.len() as u32,
                            clock + 2 * track.repeat() as u32,
                            slot as u32,
                            T::WIDTH,
                        ),
                    });
                    self.records.extend(track.keys.iter().map(|key| GpuTrackRecord {
                        data: key.value.to_vec4() + offset,
                        info: UVec4::new(key.time.to_bits(), key.ease as u32, 0, 0),
                    }));
                    self.track_count += 1;
                    return;
                }
                None
            }
        };
        match value {
            Some(value) => {
                let width = T::WIDTH as usize;
                self.values[slot..slot + width]
                    .copy_from_slice(&(value + offset).to_array()[..width]);
            }
            None => self.valid = false,
        }
    }

    /// Writes the next parameter as a plain value that can't be animated.
    pub fn put_raw<T: Keyable>(&mut self, value: T) {
        let width = T::WIDTH as usize;
        let slot = self.next_slot(width);
        self.values[slot..slot + width].copy_from_slice(&value.to_vec4().to_array()[..width]);
    }

    /// (`a`, `b`, track count), or `None` if a parameter's track was empty.
    pub fn finish(self) -> Option<(Vec4, Vec4, u32)> {
        assert_eq!(self.layout.len(), 0, "fewer parameters than the kind defines");
        self.valid.then(|| {
            (
                Vec4::from_slice(&self.values[..4]),
                Vec4::from_slice(&self.values[4..]),
                self.track_count,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(track: &Track<f32>, t: f32) -> f32 {
        track.sample(t).unwrap().x
    }

    #[test]
    fn interpolates_sorted_keys_and_holds_the_ends() {
        let track = Track::over_life([(0.75, 4.0), (0.25, 2.0)]);
        for (t, expected) in [(0.0, 2.0), (0.25, 2.0), (0.5, 3.0), (0.75, 4.0), (1.0, 4.0)] {
            assert_eq!(at(&track, t), expected, "at {t}");
        }
    }

    #[test]
    fn eases_per_key() {
        let track = Track::over_life([
            (0.0, 0.0, Ease::Step),
            (1.0, 4.0, Ease::Smooth),
            (2.0, 8.0, Ease::Linear),
        ]);
        assert_eq!(at(&track, 0.99), 0.0);
        assert_eq!(at(&track, 1.0), 4.0);
        assert_eq!(at(&track, 1.25), 4.0 + 4.0 * 0.15625);
    }

    #[test]
    fn keys_at_the_same_time_cut() {
        let track = Track::over_life([(0.0, 0.0), (0.5, 1.0), (0.5, 10.0), (1.0, 10.0)]);
        assert!(at(&track, 0.4999) < 1.0);
        assert_eq!(at(&track, 0.5), 10.0);
    }

    #[test]
    fn repeats() {
        let track = Track::over_seconds([(0.0, 0.0), (2.0, 2.0)]);
        assert_eq!(at(&track, 2.5), 2.0);
        assert_eq!(at(&track.clone().looped(), 2.5), 0.5);
        assert_eq!(at(&track.clone().ping_pong(), 2.5), 1.5);
        assert_eq!(at(&track.ping_pong(), 4.5), 0.5);
        // A single key at 0 has no period.
        assert_eq!(at(&Track::over_seconds([(0.0, 7.0)]).looped(), 3.0), 7.0);
    }

    #[test]
    fn empty_tracks_have_no_value() {
        assert!(Track::<f32>::over_time(Vec::<(f32, f32)>::new()).sample(0.0).is_none());
    }
}
