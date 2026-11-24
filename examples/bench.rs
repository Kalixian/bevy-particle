//! GPU time of the particle passes: `cargo run --release --example bench -- <capacity> <live>
//! [gravity|churn] [emitters]`
//!
//! Prints average milliseconds after a few seconds and exits. With `gravity`, all live
//! particles attract each other (Barnes-Hut); with `churn`, they live for a second and are
//! constantly replaced. `emitters` splits them among that many. Needs GPU timestamps (Vulkan or DX12).

use bevy::{
    diagnostic::DiagnosticsStore, prelude::*, render::diagnostic::RenderDiagnosticsPlugin,
    window::PresentMode,
};
use bevy_particle::{
    Fill, Gravity, GravityMethod, ParticleEmitter, ParticlePlugin, ParticleSpace, SpawnArea,
};

fn main() {
    let window = Window { present_mode: PresentMode::AutoNoVsync, ..default() };
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin { primary_window: Some(window), ..default() }),
            RenderDiagnosticsPlugin,
            ParticlePlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, report)
        .run();
}

fn setup(mut commands: Commands) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |i: usize, default| args.get(i).map_or(default, |a: &String| a.parse().unwrap());
    let capacity = number(0, 1_000_000);
    let live = number(1, 1000);
    let mode = args.get(2).map_or("", String::as_str);

    let emitters = number(3, 1);

    commands.spawn(Camera2d);
    let churn = mode == "churn";
    let emitter = ParticleEmitter {
        burst_count: if churn { 0 } else { live / emitters },
        rate: if churn { (live / emitters) as f32 } else { 0.0 },
        lifetime: churn.then(|| (1.0..1.0).into()),
        spawn: SpawnArea::Rect { half_extents: Vec2::splat(300.0), fill: Fill::Area },
        radius: (0.5..0.5).into(),
        speed: (0.0..1.0).into(),
        gravity: mode == "gravity",
        ..default()
    };
    let mut space = commands.spawn(ParticleSpace { capacity });
    space.with_children(|space| {
        for _ in 0..emitters {
            space.spawn(emitter.clone());
        }
    });
    if mode == "gravity" {
        let method = GravityMethod::BarnesHut;
        space.insert(Gravity { strength: 0.01, softening: 5.0, method, theta: 0.5 });
    }
}

fn report(time: Res<Time<Real>>, store: Res<DiagnosticsStore>, mut exit: MessageWriter<AppExit>) {
    if time.elapsed_secs() < 6.0 {
        return;
    }
    let mut rows: Vec<_> = store
        .iter()
        .filter_map(|d| {
            let name = d.path().as_str().strip_prefix("render/")?.strip_suffix("/elapsed_gpu")?;
            let ours = name.starts_with("particles") || name.contains("transparent");
            ours.then_some((name.to_string(), d.average()?))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, ms) in rows {
        println!("{name:<45} {ms:8.3}");
    }
    exit.write(AppExit::Success);
}
