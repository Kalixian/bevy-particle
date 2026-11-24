//! Drawing options: `cargo run --example render`
//!
//! Left to right: soft additive glow, sparks stretched along their motion, and an animated
//! texture. The gray sprite is in front of the first space and behind the others. `V` hides or
//! shows the sparks; the camera doesn't see a fourth space, on another render layer.

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::RenderLayers,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use bevy_particle::{
    Blend, BlendMode, Modifier, Modifiers, ParticleEmitter, ParticlePlugin, ParticleRender,
    ParticleSpace, ParticleTexture, Playback, Track,
};

#[derive(Component)]
struct Sparks;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ParticlePlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, toggle_sparks)
        .run();
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.spawn(Camera2d);
    commands.spawn((
        Sprite::from_color(Color::srgb(0.3, 0.3, 0.3), Vec2::new(1000.0, 60.0)),
        Transform::from_xyz(0.0, 0.0, 1.0),
    ));

    let fade = Modifier::Alpha(
        Blend::Scale,
        Track::over_life([(0.0, 1.0), (1.0, 0.0)]).into(),
    );
    let fire = ParticleEmitter {
        rate: 200.0,
        speed: (20.0..60.0).into(),
        radius: (15.0..30.0).into(),
        red: (1.0..1.0).into(),
        green: (0.3..0.5).into(),
        blue: (0.1..0.1).into(),
        alpha: (0.2..0.3).into(),
        ..default()
    };
    commands.spawn((
        ParticleSpace { capacity: 2000 },
        ParticleRender { blend: BlendMode::Additive, softness: 1.0, ..default() },
        Transform::from_xyz(-350.0, 0.0, 0.0),
        children![(fire.clone(), Modifiers(vec![fade.clone()]))],
    ));

    commands.spawn((
        Sparks,
        Visibility::Visible, // `toggle_visible_hidden` ignores the default, `Inherited`.
        ParticleSpace { capacity: 2000 },
        ParticleRender { blend: BlendMode::Additive, stretch: 0.1, ..default() },
        Modifiers(vec![
            Modifier::Acceleration(Vec2::new(0.0, -300.0).into()),
            Modifier::Bounds { half_extents: Vec2::new(150.0, 200.0).into(), restitution: 0.5.into() },
        ]),
        Transform::from_xyz(0.0, 0.0, 2.0),
        children![(
            ParticleEmitter {
                rate: 150.0,
                speed: (100.0..300.0).into(),
                green: (0.7..0.9).into(),
                blue: (0.3..0.3).into(),
                ..default()
            },
            Modifiers(vec![fade]),
        )],
    ));

    commands.spawn((
        ParticleSpace { capacity: 200 },
        ParticleRender {
            texture: Some(ParticleTexture {
                image: images.add(number_sheet()),
                columns: 2,
                rows: 2,
                playback: Playback::Fps(2.0),
            }),
            ..default()
        },
        Transform::from_xyz(350.0, 0.0, 2.0),
        children![ParticleEmitter {
            rate: 10.0,
            speed: (40.0..80.0).into(),
            radius: (12.0..20.0).into(),
            ..default()
        }],
    ));

    commands.spawn((
        ParticleSpace { capacity: 2000 },
        RenderLayers::layer(1),
        children![fire],
    ));
}

fn toggle_sparks(
    keys: Res<ButtonInput<KeyCode>>,
    mut sparks: Single<&mut Visibility, With<Sparks>>,
) {
    if keys.just_pressed(KeyCode::KeyV) {
        sparks.toggle_visible_hidden();
    }
}

/// A 2x2 sheet whose frames show one to four bars, each frame in its own color.
fn number_sheet() -> Image {
    const FRAME: usize = 32;
    let colors = [[255, 80, 80], [80, 255, 80], [80, 120, 255], [255, 255, 80]];
    let mut data = Vec::new();
    for y in 0..FRAME * 2 {
        for x in 0..FRAME * 2 {
            let frame = y / FRAME * 2 + x / FRAME;
            let bar = x % FRAME / 8;
            let inside = bar <= frame && x % 8 < 6 && (4..28).contains(&(y % FRAME));
            let [r, g, b] = colors[frame];
            data.extend([r, g, b, if inside { 255 } else { 40 }]);
        }
    }
    let side = FRAME as u32 * 2;
    Image::new(
        Extent3d { width: side, height: side, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}
