use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    post_process::bloom::{Bloom, BloomCompositeMode},
    mesh::MeshVertexBufferLayoutRef,
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState,
        RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
    sprite_render::{AlphaMode2d, Material2d, Material2dKey, Material2dPipeline, Material2dPlugin},
};

#[derive(Component)]
struct Velocity(Vec2);

#[derive(Component)]
struct Mass(f32);

/// Unlit material that adds `color.rgb * color.a` onto whatever is behind it.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct AdditiveMaterial {
    #[uniform(0)]
    color: LinearRgba,
}

impl Material2d for AdditiveMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/additive.wesl".into()
    }

    // Puts the mesh in the transparent phase (no depth write, drawn after opaque).
    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }

    // Replace the standard alpha blend with additive blending.
    fn specialize(
        _pipeline: &Material2dPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState {
                    // result.rgb = src.rgb * src.a + dst.rgb
                    color: BlendComponent {
                        src_factor: BlendFactor::SrcAlpha,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                    // leave destination alpha untouched
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                });
            }
        }
        Ok(())
    }
}

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, Material2dPlugin::<AdditiveMaterial>::default()))
        .add_systems(Startup, setup)
        .add_systems(Update, update_particles)
        .run();
}


fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<AdditiveMaterial>>,
    asset_server: Res<AssetServer>,
) {
    commands.spawn((
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        Tonemapping::TonyMcMapface, // 1. Using a tonemapper that desaturates to white is recommended
        Bloom {
            composite_mode: BloomCompositeMode::Additive,
            intensity: 0.3,
            low_frequency_boost: 0.5,
            ..Bloom::NATURAL
        },           // 2. Enable bloom for the camera
        DebandDither::Enabled,      // Optional: bloom causes gradients which cause banding
    ));

    let count = 2000;
    // One shared circle mesh for every particle
    let circle = meshes.add(Circle::new(2.));
    for _i in 0..count{
        let x: f32 = rand::random_range(-200.0..200.0);
        let y: f32 = rand::random_range(-200.0..200.0);
        let angle: f32 = rand::random_range(0.0..std::f32::consts::TAU);
        let speed: f32 = rand::random_range(5.0..50.0);
        let red: f32 = rand::random_range(6.0..9.0);
        let blue: f32 = rand::random_range(6.0..9.0);
        let alpha: f32 = rand::random_range(0.1..0.3);

        commands.spawn((
            Mesh2d(circle.clone()),
            // Bright HDR color (for bloom); alpha scales how much each particle adds
            MeshMaterial2d(materials.add(AdditiveMaterial {
                color: LinearRgba::new(red, 0.0, blue, alpha),
            })),
            Transform::from_translation(Vec3::new(x, y, 0.)),
            Velocity(Vec2::new(angle.cos() * speed, angle.sin() * speed)),
            Mass(1.0),
        ));
    }
}

fn update_particles(
    time: Res<Time>,
    mut query: Query<(&mut Transform, &mut Velocity), With<Mesh2d>>,
) {
    for (mut transform, mut velocity) in &mut query {
        transform.translation.x += velocity.0.x * time.delta_secs();
        transform.translation.y += velocity.0.y * time.delta_secs();

        let distance_from_center = transform.translation.length();
        let pull_max: f32 = 300000.0;
        let mut pull_strength: f32 = 0.0;
        if distance_from_center > 0.0 {
            pull_strength = pull_max / (distance_from_center * distance_from_center).max(1.0);
            pull_strength = pull_strength.clamp(0.0, 100.0);
        }
        let normalized_direction = -transform.translation.truncate().normalize_or_zero();
        velocity.0 += normalized_direction * pull_strength * time.delta_secs();
    }
}


