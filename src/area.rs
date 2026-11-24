//! Where an emitter's particles start, and which way they set off.

use std::f32::consts::TAU;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::spread::{GpuCurve, Spread, NO_CURVE};

/// The area an emitter spawns particles in, centered on the emitter and moved, rotated and
/// scaled by its `Transform`. Particles are spread evenly over it, unless noted otherwise.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SpawnArea {
    #[default]
    Point,
    Rect {
        half_extents: Vec2,
        #[serde(default)]
        fill: Fill,
    },
    Ellipse {
        radii: Vec2,
        #[serde(default)]
        fill: Fill,
    },
    /// A ring, disk (`radius: 0.0..r`), or a slice of either.
    Ring {
        /// Uniform covers the area evenly; other shapes favor smaller or larger radii.
        radius: Spread,
        /// Radians counter-clockwise from the emitter's +X. All around if left out of a file.
        #[serde(default = "full_circle")]
        arc: Spread,
    },
    /// Both ends are relative to the emitter.
    Line { from: Vec2, to: Vec2 },
}

/// Which part of a [`SpawnArea`] particles spawn in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fill {
    #[default]
    Area,
    /// Only the outline.
    Edge,
}

/// The direction that an emitter's `angle` is measured from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnDirection {
    /// The emitter's +X, which its `Transform` can rotate.
    #[default]
    Fixed,
    /// Away from the emitter. Particles that spawn right on it use `Normal`.
    Outward,
    /// Straight out of the nearest side of a `Rect`, or out through the curve of an `Ellipse` or
    /// `Ring`. For a `Line`, to the left going `from` -> `to`; for a `Point`, the same as `Fixed`.
    Normal,
}

impl SpawnDirection {
    /// Must match the `DIRECTION_*` constants in `particle_sim.wesl`.
    pub(crate) fn id(self) -> u32 {
        self as u32
    }
}

fn full_circle() -> Spread {
    (0.0..TAU).into()
}

// Must match the same constants in `particle_sim.wesl`.
pub(crate) const AREA_POINT: u32 = 0;
pub(crate) const AREA_RECT: u32 = 1;
pub(crate) const AREA_RECT_EDGE: u32 = 2;
pub(crate) const AREA_ELLIPSE: u32 = 3;
pub(crate) const AREA_ELLIPSE_EDGE: u32 = 4;
pub(crate) const AREA_RING: u32 = 5;
pub(crate) const AREA_LINE: u32 = 6;

/// A [`SpawnArea`] as stored in `GpuEmitter`.
#[derive(Debug, PartialEq)]
pub(crate) struct AreaData {
    pub kind: u32,
    /// Half extents or radii in xy; `from`, `to`; or radius² range, arc range.
    pub params: Vec4,
    /// A ring's radius and arc curves.
    pub curves: UVec2,
}

impl SpawnArea {
    pub(crate) fn to_gpu(&self, curves: &mut Vec<GpuCurve>) -> AreaData {
        let plain = |kind, a: Vec2, b: Vec2| AreaData {
            kind,
            params: Vec4::new(a.x, a.y, b.x, b.y),
            curves: UVec2::splat(NO_CURVE),
        };
        match self {
            Self::Point => plain(AREA_POINT, Vec2::ZERO, Vec2::ZERO),
            Self::Rect { half_extents, fill } => {
                let kind = if *fill == Fill::Edge { AREA_RECT_EDGE } else { AREA_RECT };
                plain(kind, half_extents.abs(), Vec2::ZERO)
            }
            Self::Ellipse { radii, fill } => {
                let kind = if *fill == Fill::Edge { AREA_ELLIPSE_EDGE } else { AREA_ELLIPSE };
                plain(kind, radii.abs(), Vec2::ZERO)
            }
            Self::Ring { radius, arc } => {
                let (radius, radius_curve) = radius.to_gpu(curves);
                let (arc, arc_curve) = arc.to_gpu(curves);
                // Squared, since area grows with the square of the radius.
                let radius = radius.max(Vec2::ZERO);
                AreaData {
                    curves: UVec2::new(radius_curve, arc_curve),
                    ..plain(AREA_RING, radius * radius, arc)
                }
            }
            Self::Line { from, to } => plain(AREA_LINE, *from, *to),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spread::Shape;

    fn parse(text: &str) -> SpawnArea {
        ron::from_str(text).unwrap()
    }

    #[test]
    fn fill_and_arc_are_optional_in_files() {
        let mut curves = Vec::new();
        let rect = parse("Rect(half_extents: (1.0, 2.0))").to_gpu(&mut curves);
        assert_eq!((rect.kind, rect.params), (AREA_RECT, Vec4::new(1.0, 2.0, 0.0, 0.0)));
        let ellipse = parse("Ellipse(radii: (1.0, 2.0), fill: Edge)").to_gpu(&mut curves);
        assert_eq!(ellipse.kind, AREA_ELLIPSE_EDGE);
        let ring = parse("Ring(radius: (from: 2.0, to: 3.0))").to_gpu(&mut curves);
        assert_eq!((ring.kind, ring.params), (AREA_RING, Vec4::new(4.0, 9.0, 0.0, TAU)));
        assert!(ron::from_str::<SpawnArea>("Rect(half_extents: (1.0, 2.0), fil: Edge)").is_err());
    }

    #[test]
    fn rings_use_the_curves_of_their_spreads() {
        let mut curves = vec![GpuCurve::default()];
        let ring = SpawnArea::Ring {
            radius: (0.0..1.0).into(),
            arc: Spread::new(0.0..1.0, Shape::Low(2.0)),
        };
        assert_eq!(ring.to_gpu(&mut curves).curves, UVec2::new(NO_CURVE, 1));
        assert_eq!(curves.len(), 2);
    }
}
