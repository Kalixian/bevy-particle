//! The single definition of every modifier kind: its ID, when it runs, and how its parameters
//! are laid out. The Rust encoder and the shader both follow it; the shader through a generated
//! module with, for each kind,
//!
//! - `const MOD_POINT_ATTRACTOR: u32`, its ID;
//! - `struct PointAttractorParams`, its parameters;
//! - `fn point_attractor_params(m: Modifier) -> PointAttractorParams`, which unpacks them.

use std::fmt::Write;

/// Where the generated module lives among the embedded shaders.
pub(crate) const WESL_PATH: &str = "embedded://bevy_particle/gpu/shaders/modifier_kinds.wesl";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModifierKind {
    Acceleration,
    PointAttractor,
    Vortex,
    Drag,
    Bounds,
    Color,
    Alpha,
    Size,
    Mass,
}

/// When a modifier runs. Within a phase, the space's modifiers run before the emitter's, each
/// in list order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Changes velocity.
    Force,
    /// Limits the motion that all forces (and gravity) add up to.
    Constraint,
    /// Changes color, size and mass.
    Appearance,
}

impl Phase {
    /// In the order they run.
    pub const ALL: [Self; 3] = [Self::Force, Self::Constraint, Self::Appearance];
}

pub(crate) struct KindDef {
    kind: ModifierKind,
    name: &'static str,
    pub phase: Phase,
    pub params: &'static [ParamDef],
}

pub(crate) struct ParamDef {
    name: &'static str,
    /// In floats.
    pub width: usize,
}

const fn param(name: &'static str, width: usize) -> ParamDef {
    ParamDef { name, width }
}

/// In order of [`ModifierKind`]. Parameters have eight floats in all.
const KINDS: &[KindDef] = {
    use ModifierKind::*;
    use Phase::*;
    const CENTERED_PULL: &[ParamDef] =
        &[param("center", 2), param("strength", 1), param("max_accel", 1)];
    // `scale`: 1 to scale the current value, 0 to replace it.
    const BLENDED_FLOAT: &[ParamDef] = &[param("value", 1), param("scale", 1)];
    const BLENDED_RGB: &[ParamDef] = &[param("value", 3), param("scale", 1)];

    const fn def(
        kind: ModifierKind,
        name: &'static str,
        phase: Phase,
        params: &'static [ParamDef],
    ) -> KindDef {
        KindDef { kind, name, phase, params }
    }
    &[
        def(Acceleration, "acceleration", Force, &[param("accel", 2)]),
        def(PointAttractor, "point_attractor", Force, CENTERED_PULL),
        def(Vortex, "vortex", Force, CENTERED_PULL),
        def(Drag, "drag", Force, &[param("coefficient", 1)]),
        def(
            Bounds,
            "bounds",
            Constraint,
            &[param("center", 2), param("half_extents", 2), param("restitution", 1)],
        ),
        def(Color, "color", Appearance, BLENDED_RGB),
        def(Alpha, "alpha", Appearance, BLENDED_FLOAT),
        def(Size, "size", Appearance, BLENDED_FLOAT),
        def(Mass, "mass", Appearance, BLENDED_FLOAT),
    ]
};

impl ModifierKind {
    pub fn id(self) -> u32 {
        self as u32
    }

    pub fn def(self) -> &'static KindDef {
        let def = &KINDS[self as usize];
        debug_assert_eq!(def.kind, self, "KINDS is out of order");
        def
    }
}

/// `point_attractor` -> `PointAttractor`.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .map(|word| word[..1].to_uppercase() + &word[1..])
        .collect()
}

/// The source of the generated shader module.
pub(crate) fn wesl_source() -> String {
    const FLOATS: [&str; 8] =
        ["m.a.x", "m.a.y", "m.a.z", "m.a.w", "m.b.x", "m.b.y", "m.b.z", "m.b.w"];

    let mut out = String::from("import package::gpu::shaders::types::Modifier;\n");
    for KindDef { kind, name, params, .. } in KINDS {
        let (id, constant, ty) = (kind.id(), name.to_uppercase(), pascal_case(name));
        let mut fields = String::new();
        let mut values = Vec::new();
        let mut slot = 0;
        for ParamDef { name, width } in *params {
            let floats = FLOATS[slot..slot + width].join(", ");
            slot += width;
            let (ty, value) = match width {
                1 => ("f32".to_string(), floats),
                _ => (format!("vec{width}<f32>"), format!("vec{width}<f32>({floats})")),
            };
            writeln!(fields, "    {name}: {ty},").unwrap();
            values.push(value);
        }
        let values = values.join(", ");
        write!(
            out,
            "
const MOD_{constant}: u32 = {id}u;
struct {ty}Params {{
{fields}}}
fn {name}_params(m: Modifier) -> {ty}Params {{
    return {ty}Params({values});
}}
"
        )
        .unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_in_order_and_fit() {
        for (i, def) in KINDS.iter().enumerate() {
            assert_eq!(def.kind.id(), i as u32, "{}", def.name);
            let floats: usize = def.params.iter().map(|p| p.width).sum();
            assert!(floats <= 8, "{}", def.name);
        }
    }

    #[test]
    fn generates_readers_for_each_kind() {
        let source = wesl_source();
        for expected in [
            "const MOD_POINT_ATTRACTOR: u32 = 1u;",
            "struct PointAttractorParams {\n    center: vec2<f32>,\n    strength: f32,",
            "fn bounds_params(m: Modifier) -> BoundsParams {",
            "return BoundsParams(vec2<f32>(m.a.x, m.a.y), vec2<f32>(m.a.z, m.a.w), m.b.x);",
        ] {
            assert!(source.contains(expected), "{source}");
        }
    }
}
