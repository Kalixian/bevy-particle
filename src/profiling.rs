//! GPU timing for the particle passes. Enabled with `cargo run --features profile`.
//!
//! Every 2 seconds, logs the average GPU time of each `particles/...` span recorded in
//! `gpu/compute.rs` and `gpu/gravity.rs`, plus the overall frame time. Vsync is turned off
//! in this mode (see `main.rs`) so frame times aren't capped.
//!
//! GPU timestamps need Vulkan or DX12; elsewhere only CPU times are recorded and the table
//! comes out empty.

use std::time::Duration;

use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
    render::diagnostic::RenderDiagnosticsPlugin,
    time::common_conditions::on_timer,
};

pub struct ProfilingPlugin;

impl Plugin for ProfilingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RenderDiagnosticsPlugin, FrameTimeDiagnosticsPlugin::default()))
            .add_systems(Update, report.run_if(on_timer(Duration::from_secs(2))));
    }
}

fn report(store: Res<DiagnosticsStore>) {
    let mut spans: Vec<(String, f64)> = store
        .iter()
        .filter_map(|diagnostic| {
            let path = diagnostic.path().as_str();
            let name = path
                .strip_prefix("render/particles")?
                .strip_suffix("/elapsed_gpu")?;
            let name = name.trim_start_matches('/');
            let name = if name.is_empty() { "total" } else { name };
            Some((name.to_string(), diagnostic.average()?))
        })
        .collect();
    if spans.is_empty() {
        return;
    }
    spans.sort_by(|a, b| a.0.cmp(&b.0));

    let frame_ms = store
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.average())
        .unwrap_or(0.0);
    let table: String = spans
        .iter()
        .map(|(name, ms)| format!("\n  {name:<24} {ms:>7.3} ms"))
        .collect();
    info!("Particle GPU time (frame {frame_ms:.2} ms):{table}");
}
