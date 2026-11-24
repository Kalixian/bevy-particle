//! Shaped random ranges for emitter spawn values.
//!
//! A [`Spread`] is a range plus a [`Shape`] that controls how values are distributed across
//! it. Each spawn draws a uniform random `u` in [0, 1], the shape bends it into a position
//! `t` in [0, 1], and the value is `start + (end - start) · t`. A shape is the inverse of a
//! cumulative distribution: the steeper it climbs over some stretch of `u`, the rarer the
//! values it maps there.
//!
//! On the GPU every non-uniform shape is a 32-point lookup table, so built-in and custom
//! shapes cost the same.

use std::ops::Range;

use bevy::{prelude::*, render::render_resource::ShaderType};
use serde::{Deserialize, Serialize};

use crate::util::is_default;

/// Points per lookup table. Must match `CURVE_SAMPLES` in `particle_sim.wesl`.
pub const CURVE_SAMPLES: usize = 32;
/// Curve index meaning "uniform, no table". Must match `NO_CURVE` in `particle_sim.wesl`.
pub const NO_CURVE: u32 = u32::MAX;

/// A random range with a distribution shape.
///
/// ```ignore
/// radius: (0.5..2.0).into(),                              // uniform
/// radius: Spread::new(0.5..8.0, Shape::Falloff(6.0)),    // mostly small, a few large
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "SpreadData", into = "SpreadData")]
pub struct Spread {
    pub range: Range<f32>,
    pub shape: Shape,
}

/// A [`Spread`] as written in data files: `(from: 0.5, to: 2.0, shape: Low(3.0))`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpreadData {
    from: f32,
    to: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    shape: Shape,
}

impl From<SpreadData> for Spread {
    fn from(data: SpreadData) -> Self {
        Self::new(data.from..data.to, data.shape)
    }
}

impl From<Spread> for SpreadData {
    fn from(spread: Spread) -> Self {
        Self { from: spread.range.start, to: spread.range.end, shape: spread.shape }
    }
}

impl Spread {
    pub fn new(range: Range<f32>, shape: Shape) -> Self {
        Self { range, shape }
    }

    /// The range's (start, end) and its curve's index in `curves`, appending the curve's
    /// lookup table if it has one.
    pub(crate) fn to_gpu(&self, curves: &mut Vec<GpuCurve>) -> (Vec2, u32) {
        let range = Vec2::new(self.range.start, self.range.end);
        if matches!(self.shape, Shape::Uniform) {
            return (range, NO_CURVE);
        }
        curves.push(GpuCurve::tabulate(&self.shape));
        (range, curves.len() as u32 - 1)
    }
}

impl From<Range<f32>> for Spread {
    fn from(range: Range<f32>) -> Self {
        Self::new(range, Shape::Uniform)
    }
}

/// How values are distributed across a [`Spread`]'s range. "Start" and "end" refer to the
/// range as written, so `Low` on `8.0..0.5` favors 8.0.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum Shape {
    /// Every value equally likely.
    #[default]
    Uniform,
    /// Favors the start: `t = u^power`. Higher power, stronger bias (2 = mild, 6 = strong).
    Low(f32),
    /// Favors the end: `t = 1 - (1 - u)^power`.
    High(f32),
    /// Bell-like, favoring the middle; higher power narrows it.
    Middle(f32),
    /// Exponential falloff from the start: most values near the start with a long thin tail
    /// of large ones, like sizes in nature. Higher rate makes the tail rarer (3 = gentle,
    /// 8 = few outliers). The full range is still reachable.
    Falloff(f32),
    /// Your own mapping from `u` in [0, 1] to `t` in [0, 1]. It should rise from 0 to 1;
    /// results are clamped to [0, 1]. A plain `fn` or a closure that captures nothing:
    /// `Shape::Custom(|u| u * u * (3.0 - 2.0 * u))`.
    ///
    /// Code only: it can't be loaded from or saved to a data file.
    #[serde(skip)]
    Custom(fn(f32) -> f32),
}

/// Custom shapes are equal if they're the same function, as far as that can be told.
impl PartialEq for Shape {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Uniform, Self::Uniform) => true,
            (Self::Low(a), Self::Low(b))
            | (Self::High(a), Self::High(b))
            | (Self::Middle(a), Self::Middle(b))
            | (Self::Falloff(a), Self::Falloff(b)) => a == b,
            (Self::Custom(a), Self::Custom(b)) => std::ptr::fn_addr_eq(*a, *b),
            _ => false,
        }
    }
}

impl Shape {
    /// Maps uniform `u` in [0, 1] to a position `t` in [0, 1] along the range.
    pub fn apply(&self, u: f32) -> f32 {
        let t = match *self {
            Shape::Uniform => u,
            Shape::Low(power) => u.powf(power.max(1e-3)),
            Shape::High(power) => 1.0 - (1.0 - u).powf(power.max(1e-3)),
            Shape::Middle(power) => {
                let x = 2.0 * u - 1.0;
                0.5 + 0.5 * x.signum() * x.abs().powf(power.max(1e-3))
            }
            Shape::Falloff(rate) => {
                if rate.abs() < 1e-4 {
                    u
                } else {
                    // Inverse CDF of an exponential truncated to [0, 1].
                    -(1.0 - u * (1.0 - (-rate).exp())).ln() / rate
                }
            }
            Shape::Custom(f) => f(u),
        };
        t.clamp(0.0, 1.0)
    }
}

/// A shape's lookup table: `t` at `u = i / (CURVE_SAMPLES - 1)`, packed four per `Vec4`.
/// Must match `struct Curve` in `particle_sim.wesl`.
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuCurve {
    samples: [Vec4; CURVE_SAMPLES / 4],
}

impl GpuCurve {
    fn tabulate(shape: &Shape) -> Self {
        let mut samples = [Vec4::ZERO; CURVE_SAMPLES / 4];
        for i in 0..CURVE_SAMPLES {
            let u = i as f32 / (CURVE_SAMPLES - 1) as f32;
            samples[i / 4][i % 4] = shape.apply(u);
        }
        Self { samples }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [Shape; 6] = [
        Shape::Uniform,
        Shape::Low(3.0),
        Shape::High(3.0),
        Shape::Middle(3.0),
        Shape::Falloff(6.0),
        Shape::Custom(|u| u * u),
    ];

    #[test]
    fn shapes_span_the_whole_range_in_order() {
        for shape in SHAPES {
            assert!(shape.apply(0.0).abs() < 1e-5, "{shape:?} at 0");
            assert!((shape.apply(1.0) - 1.0).abs() < 1e-5, "{shape:?} at 1");
            let mut previous = 0.0;
            for i in 0..=100 {
                let t = shape.apply(i as f32 / 100.0);
                assert!(t >= previous - 1e-6, "{shape:?} decreases at u = {i}%");
                previous = t;
            }
        }
    }

    #[test]
    fn falloff_is_mostly_small_with_a_rare_large_tail() {
        // The scene's cluster radius: 0.4..5.6 with Falloff(6).
        let spread = Spread::new(0.4..5.6, Shape::Falloff(6.0));
        let value = |u: f32| {
            let t = spread.shape.apply(u);
            spread.range.start + (spread.range.end - spread.range.start) * t
        };
        let median = value(0.5);
        let p99 = value(0.99);
        assert!((0.9..1.1).contains(&median), "median {median}");
        assert!((4.2..4.6).contains(&p99), "99th percentile {p99}");
    }

    #[test]
    fn lookup_table_matches_the_shape() {
        let shape = Shape::Falloff(6.0);
        let table = GpuCurve::tabulate(&shape);
        for i in 0..CURVE_SAMPLES {
            let u = i as f32 / (CURVE_SAMPLES - 1) as f32;
            assert_eq!(table.samples[i / 4][i % 4], shape.apply(u));
        }
    }
}
