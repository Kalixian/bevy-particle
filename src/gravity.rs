use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Most particles that can take part in gravity. Set by the radix sort's two-level scan
/// (16 digits × tiles of 256 must fit in 256 blocks of 256). Extra participants are left out.
pub const MAX_GRAVITY_BODIES: u32 = 1 << 20;

/// N-body gravity between the space's particles. Add it to the space entity to enable it.
///
/// Only particles whose emitter has `gravity: true` take part: they attract each other and
/// feel each other's pull. Other particles ignore gravity entirely.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gravity {
    /// Gravitational constant G: acceleration = G · mass / distance².
    pub strength: f32,
    /// Plummer softening length ε: distances are treated as √(d² + ε²), which keeps close
    /// encounters from producing huge accelerations.
    pub softening: f32,
    pub method: GravityMethod,
    /// Barnes-Hut opening angle θ: a tree node counts as a single mass once
    /// `node size / distance < θ`. Smaller is more accurate and slower; 0.5 is typical.
    pub theta: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GravityMethod {
    /// Exact, O(n²). Practical up to ~100k participants.
    BruteForce,
    /// Approximate, O(n log n). Up to [`MAX_GRAVITY_BODIES`] participants.
    BarnesHut,
}

impl GravityMethod {
    pub(crate) fn as_gpu(self) -> u32 {
        match self {
            GravityMethod::BruteForce => 0,
            GravityMethod::BarnesHut => 1,
        }
    }
}
