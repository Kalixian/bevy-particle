//! Compiles every shader the way Bevy does (imports included), validates the result with naga,
//! and checks that declarations shared with Rust match. No GPU needed.

use std::{any::type_name, fmt::Debug, sync::Arc};

use bevy::{
    asset::{uuid::Uuid, AssetId},
    render::render_resource::{
        encase::private::StructMetadata,
        ShaderType,
    },
    shader::{Shader, ShaderCache, ShaderCacheError, ShaderCacheSource, ValidateShader},
};
use naga::{
    valid::{Capabilities, ValidationFlags, Validator},
    Module, TypeInner,
};

use super::gravity::RadixPassParams;
use crate::{
    area::{self, SpawnDirection},
    emitter::GpuEmitter,
    modifier::GpuModifier,
    modifier_kinds,
    particle::GpuParticle,
    render::GpuRenderParams,
    space::SimParams,
    spread::{GpuCurve, CURVE_SAMPLES, NO_CURVE},
    track::GpuTrackRecord,
};

macro_rules! shaders {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("shaders/", $name, ".wesl")))),*]
    };
}

const SHADERS: &[(&str, &str)] = shaders![
    "types",
    "random",
    "scan",
    "tracks",
    "particle_sim",
    "particle_draw",
    "live",
    "gravity_args",
    "gravity_brute",
    "gravity_bh",
];

fn load_module(
    _: &(),
    source: ShaderCacheSource,
    _: &ValidateShader,
) -> Result<Module, ShaderCacheError> {
    let ShaderCacheSource::Wgsl(source) = source else {
        panic!("expected WGSL");
    };
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    // For `unpack2x16float`, which every WebGPU device has. Nothing else beyond the baseline.
    let capabilities = Capabilities::SHADER_FLOAT16_IN_FLOAT32;
    Validator::new(ValidationFlags::all(), capabilities)
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    Ok(module)
}

fn compile(name: &str) -> Arc<Module> {
    let id = |i: usize| AssetId::<Shader>::Uuid { uuid: Uuid::from_u128(i as u128 + 1) };
    let mut cache = ShaderCache::new((), load_module);
    for (i, (name, source)) in SHADERS.iter().enumerate() {
        let path = format!("embedded://bevy_particle/gpu/shaders/{name}.wesl");
        cache.set_shader(id(i), Shader::from_wesl(*source, path));
    }
    let kinds = Shader::from_wesl(modifier_kinds::wesl_source(), modifier_kinds::WESL_PATH);
    cache.set_shader(id(SHADERS.len()), kinds);
    let index = SHADERS.iter().position(|(n, _)| *n == name).unwrap();
    cache.get(0, id(index), &[]).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Imported names come out mangled, with the module path in front.
fn is_named(mangled: Option<&str>, name: &str) -> bool {
    mangled.is_some_and(|m| m == name || m.ends_with(&format!("_{name}")))
}

/// The (field, offset) pairs and size of a struct in a compiled shader.
fn shader_layout(module: &Module, name: &str) -> (Vec<(String, u64)>, u64) {
    let mut found = module.types.iter().filter_map(|(_, ty)| match &ty.inner {
        TypeInner::Struct { members, span } if is_named(ty.name.as_deref(), name) => {
            let fields =
                members.iter().map(|m| (m.name.clone().unwrap(), m.offset as u64)).collect();
            Some((fields, *span as u64))
        }
        _ => None,
    });
    let layout = found.next().unwrap_or_else(|| panic!("no struct {name}"));
    assert!(found.next().is_none(), "more than one struct {name}");
    layout
}

/// Field names in order, from the derived `Debug` (encase only knows offsets).
fn field_names<T: Debug + Default>() -> Vec<String> {
    format!("{:#?}", T::default())
        .lines()
        .filter_map(|line| line.strip_prefix("    ")?.split_once(": "))
        .filter(|(name, _)| !name.starts_with(' '))
        .map(|(name, _)| name.to_string())
        .collect()
}

fn check_layout<T, const N: usize>(shader: &str, name: &str)
where
    T: ShaderType<ExtraMetadata = StructMetadata<N>> + Debug + Default,
{
    let names = field_names::<T>();
    assert_eq!(names.len(), N, "{}", type_name::<T>());
    let fields = names.into_iter().zip((0..N).map(|i| T::METADATA.offset(i))).collect();
    let rust: (Vec<_>, u64) = (fields, T::min_size().get());
    assert_eq!(rust, shader_layout(&compile(shader), name), "{}", type_name::<T>());
}

#[test]
fn shaders_compile_and_validate() {
    for (name, _) in SHADERS {
        let module = compile(name);
        let entry_points: Vec<_> = module.entry_points.iter().map(|e| e.name.as_str()).collect();
        let expected: &[&str] = match *name {
            "particle_sim" => &["compact", "integrate", "spawn"],
            "particle_draw" => &["vertex", "fragment"],
            "live" => &["mark", "scan_blocks", "scan_sums", "write"],
            "gravity_args" => &["size_gravity"],
            "gravity_brute" => &["gravity"],
            "gravity_bh" => &[
                "bounds_pass",
                "morton",
                "radix_histogram",
                "radix_scan_blocks",
                "radix_scan_sums",
                "radix_scatter",
                "build_tree",
                "summarize",
                "force",
            ],
            _ => &[],
        };
        assert_eq!(entry_points, expected, "{name}");
    }
}

#[test]
fn struct_layouts_match() {
    check_layout::<GpuEmitter, _>("particle_sim", "Emitter");
    check_layout::<GpuModifier, _>("particle_sim", "Modifier");
    check_layout::<GpuCurve, _>("particle_sim", "Curve");
    check_layout::<GpuTrackRecord, _>("particle_sim", "TrackRecord");
    check_layout::<GpuRenderParams, _>("particle_draw", "RenderParams");
    check_layout::<RadixPassParams, _>("gravity_bh", "RadixPass");
    for shader in ["particle_sim", "live", "gravity_args", "gravity_brute", "gravity_bh"] {
        check_layout::<SimParams, _>(shader, "SimParams");
    }
}

#[test]
fn particle_layout_matches() {
    macro_rules! offsets {
        ($($field:ident),*) => {
            vec![$((
                stringify!($field).to_string(),
                std::mem::offset_of!(GpuParticle, $field) as u64,
            )),*]
        };
    }
    let fields = offsets!(
        position, velocity, color, mass, radius, age, lifetime, emitter, base_radius, base_color,
        rotation, spin
    );
    let rust = (fields, size_of::<GpuParticle>() as u64);
    for shader in ["particle_sim", "particle_draw", "live"] {
        assert_eq!(rust, shader_layout(&compile(shader), "Particle"), "{shader}");
    }
}

#[test]
fn shared_constants_match() {
    let module = compile("particle_sim");
    let constant = |name: &str| {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| is_named(c.name.as_deref(), name))
            .unwrap_or_else(|| panic!("no constant {name}"));
        match module.global_expressions[c.init] {
            naga::Expression::Literal(naga::Literal::U32(value)) => value,
            ref other => panic!("{name} is {other:?}"),
        }
    };
    assert_eq!(constant("CURVE_SAMPLES"), CURVE_SAMPLES as u32);
    assert_eq!(constant("NO_CURVE"), NO_CURVE);
    assert_eq!(constant("AREA_RECT"), area::AREA_RECT);
    assert_eq!(constant("AREA_RECT_EDGE"), area::AREA_RECT_EDGE);
    assert_eq!(constant("AREA_ELLIPSE"), area::AREA_ELLIPSE);
    assert_eq!(constant("AREA_ELLIPSE_EDGE"), area::AREA_ELLIPSE_EDGE);
    assert_eq!(constant("AREA_RING"), area::AREA_RING);
    assert_eq!(constant("AREA_LINE"), area::AREA_LINE);
    assert_eq!(constant("DIRECTION_OUTWARD"), SpawnDirection::Outward.id());
    assert_eq!(constant("DIRECTION_NORMAL"), SpawnDirection::Normal.id());
}

#[test]
fn workgroup_sizes_match() {
    for shader in ["particle_sim", "live", "gravity_args", "gravity_brute", "gravity_bh"] {
        for entry in &compile(shader).entry_points {
            let expected = match shader {
                "particle_sim" => super::compute::WORKGROUP_SIZE,
                "live" => super::live::WORKGROUP_SIZE,
                "gravity_args" => 1,
                _ => super::gravity::WORKGROUP_SIZE,
            };
            assert_eq!(entry.workgroup_size, [expected, 1, 1], "{shader}/{}", entry.name);
        }
    }
}
