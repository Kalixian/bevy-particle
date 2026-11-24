use std::borrow::Cow;

use bevy::{
    asset::load_embedded_asset,
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{
                storage_buffer_read_only, storage_buffer_read_only_sized, storage_buffer_sized,
                uniform_buffer,
            },
            *,
        },
        diagnostic::RecordDiagnostics,
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::GpuShaderBuffer,
        Render, RenderStartup, RenderSystems,
    },
};

use super::{
    gravity::{self, GravityBindGroups, GravityBuffers, GravityPipelines},
    GpuSimReady,
};
use crate::{
    emitter::{EmitterUpload, GpuEmitter},
    modifier::{GpuModifier, ModifierUpload},
    spread::GpuCurve,
    track::GpuTrackRecord,
    particle::ParticleBuffers,
    space::SimParams,
};

/// Must match `@workgroup_size` in the shaders.
pub(super) const WORKGROUP_SIZE: u32 = 256;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SimBuffers>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(
            Render,
            (
                write_sim_buffers.in_set(RenderSystems::PrepareResources),
                prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        // Simulate before any camera renders, so every view draws this frame's state.
        .add_systems(RenderGraph, dispatch_sim.before(camera_driver));
}

#[derive(Resource)]
struct SimPipelines {
    /// `integrate` and `spawn`.
    layout: BindGroupLayoutDescriptor,
    compact_layout: BindGroupLayoutDescriptor,
    compact: CachedComputePipelineId,
    integrate: CachedComputePipelineId,
    spawn: CachedComputePipelineId,
}

/// Small per-frame buffers written from the CPU.
#[derive(Resource, Default)]
pub(super) struct SimBuffers {
    pub params: UniformBuffer<SimParams>,
    emitters: StorageBuffer<Vec<GpuEmitter>>,
    modifiers: StorageBuffer<Vec<GpuModifier>>,
    curves: StorageBuffer<Vec<GpuCurve>>,
    tracks: StorageBuffer<Vec<GpuTrackRecord>>,
}

#[derive(Resource)]
struct SimBindGroups {
    sim: BindGroup,
    compact: BindGroup,
}

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    // Each layout binds only what its entry points use, to stay within WebGPU's default limit
    // of 8 storage buffers per stage. Numbers are the `@binding`s in the shader.
    let layout = BindGroupLayoutDescriptor::new(
        "particle_sim_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<SimParams>(false)),
                // particles
                (1, storage_buffer_sized(false, None)),
                // free list
                (2, storage_buffer_sized(false, None)),
                (3, storage_buffer_read_only::<Vec<GpuEmitter>>(false)),
                (4, storage_buffer_read_only::<Vec<GpuModifier>>(false)),
                // gravity accel (read by `integrate`)
                (7, storage_buffer_read_only_sized(false, None)),
                // spread curves (read by `spawn`)
                (8, storage_buffer_read_only::<Vec<GpuCurve>>(false)),
                // modifier keyframe tracks
                (9, storage_buffer_read_only::<Vec<GpuTrackRecord>>(false)),
            ),
        ),
    );
    let compact_layout = BindGroupLayoutDescriptor::new(
        "particle_compact_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<SimParams>(false)),
                // particles
                (1, storage_buffer_sized(false, None)),
                (3, storage_buffer_read_only::<Vec<GpuEmitter>>(false)),
                // gravity bodies
                (5, storage_buffer_sized(false, None)),
                // gravity body count (atomic)
                (6, storage_buffer_sized(false, None)),
            ),
        ),
    );
    let shader: Handle<Shader> = load_embedded_asset!(asset_server.as_ref(), "shaders/particle_sim.wesl");
    let queue = |entry_point: &'static str, layout: &BindGroupLayoutDescriptor| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("particle_{entry_point}").into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(Cow::from(entry_point)),
            ..default()
        })
    };
    let compact = queue("compact", &compact_layout);
    let integrate = queue("integrate", &layout);
    let spawn = queue("spawn", &layout);
    commands.insert_resource(SimPipelines {
        layout,
        compact_layout,
        compact,
        integrate,
        spawn,
    });
}

fn write_sim_buffers(
    params: Res<SimParams>,
    emitters: Res<EmitterUpload>,
    modifiers: Res<ModifierUpload>,
    mut buffers: ResMut<SimBuffers>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    buffers.params.set(*params);
    buffers.params.write_buffer(&device, &queue);
    buffers.emitters.set(emitters.emitters.clone());
    buffers.emitters.write_buffer(&device, &queue);
    buffers.modifiers.set(modifiers.gpu_data());
    buffers.modifiers.write_buffer(&device, &queue);
    buffers.curves.set(emitters.curves_gpu_data());
    buffers.curves.write_buffer(&device, &queue);
    buffers.tracks.set(modifiers.tracks_gpu_data());
    buffers.tracks.write_buffer(&device, &queue);
}

fn prepare_bind_group(
    mut commands: Commands,
    pipelines: Res<SimPipelines>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    sim_buffers: Res<SimBuffers>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    gravity_buffers: Option<Res<GravityBuffers>>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    let gpu = particle_buffers.and_then(|b| {
        Some((
            gpu_buffers.get(&b.particles)?,
            gpu_buffers.get(&b.free_list)?,
            gpu_buffers.get(&b.accel)?,
        ))
    });
    let (
        Some((particles, free_list, accel)),
        Some(gravity),
        Some(emitters),
        Some(modifiers),
        Some(curves),
        Some(tracks),
    ) = (
        gpu,
        gravity_buffers,
        sim_buffers.emitters.binding(),
        sim_buffers.modifiers.binding(),
        sim_buffers.curves.binding(),
        sim_buffers.tracks.binding(),
    )
    else {
        commands.remove_resource::<SimBindGroups>();
        return;
    };

    let sim = device.create_bind_group(
        "particle_sim_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::with_indices((
            (0, &sim_buffers.params),
            (1, particles.buffer.as_entire_binding()),
            (2, free_list.buffer.as_entire_binding()),
            (3, emitters.clone()),
            (4, modifiers),
            (7, accel.buffer.as_entire_binding()),
            (8, curves),
            (9, tracks),
        )),
    );
    let compact = device.create_bind_group(
        "particle_compact_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.compact_layout),
        &BindGroupEntries::with_indices((
            (0, &sim_buffers.params),
            (1, particles.buffer.as_entire_binding()),
            (3, emitters),
            (5, gravity.bodies.as_entire_binding()),
            (6, gravity.body_count.as_entire_binding()),
        )),
    );
    commands.insert_resource(SimBindGroups { sim, compact });
}

fn dispatch_sim(
    mut ctx: RenderContext,
    pipelines: Res<SimPipelines>,
    gravity_pipelines: Res<GravityPipelines>,
    pipeline_cache: Res<PipelineCache>,
    bind_groups: Option<Res<SimBindGroups>>,
    gravity_bind_groups: Option<Res<GravityBindGroups>>,
    gravity_buffers: Option<Res<GravityBuffers>>,
    params: Res<SimParams>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    ready: Res<GpuSimReady>,
) {
    let (
        Some(bind_groups),
        Some(gravity_bind_groups),
        Some(gravity_buffers),
        Some(particle_buffers),
        Some(compact),
        Some(integrate),
        Some(spawn),
    ) = (
        bind_groups,
        gravity_bind_groups,
        gravity_buffers,
        particle_buffers,
        pipeline_cache.get_compute_pipeline(pipelines.compact),
        pipeline_cache.get_compute_pipeline(pipelines.integrate),
        pipeline_cache.get_compute_pipeline(pipelines.spawn),
    )
    else {
        return;
    };
    if params.capacity == 0 {
        return;
    }
    // Wait for this frame's gravity pipelines too (Barnes-Hut compiles nine of them).
    let gravity_plan = if params.gravity_enabled != 0 {
        let Some(plan) = gravity_pipelines.plan(&pipeline_cache, &params, &gravity_bind_groups)
        else {
            return;
        };
        Some(plan)
    } else {
        None
    };

    let slot_groups = params.capacity.div_ceil(WORKGROUP_SIZE);
    // GPU timing spans ("render/particles/..."); no-ops unless RenderDiagnosticsPlugin is
    // added (the `profile` feature).
    let recorder = ctx.diagnostic_recorder();
    let diagnostics = recorder.as_deref();

    let encoder = ctx.command_encoder();
    if let Some(plan) = &gravity_plan {
        gravity::clear_buffers(encoder, plan, &gravity_buffers);
    }

    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("particle_sim"),
        ..default()
    });
    let total_span = diagnostics.time_span(&mut pass, "particles");
    // Each dispatch is its own synchronization scope, so every pass sees the writes of the
    // passes before it.

    if let Some(plan) = &gravity_plan {
        let span = diagnostics.time_span(&mut pass, "compact");
        pass.set_bind_group(0, &bind_groups.compact, &[]);
        pass.set_pipeline(compact);
        pass.dispatch_workgroups(slot_groups, 1, 1);
        span.end(&mut pass);

        gravity::dispatch(&mut pass, plan, &gravity_bind_groups, &gravity_buffers, diagnostics);
    }

    pass.set_bind_group(0, &bind_groups.sim, &[]);

    // Integrate before spawn: it ages and kills particles, returning their slots to the
    // free list, so the spawn pass can reuse them in the same frame.
    let span = diagnostics.time_span(&mut pass, "integrate");
    pass.set_pipeline(integrate);
    pass.dispatch_workgroups(slot_groups, 1, 1);
    span.end(&mut pass);

    if params.spawn_total > 0 {
        let span = diagnostics.time_span(&mut pass, "spawn");
        pass.set_pipeline(spawn);
        pass.dispatch_workgroups(params.spawn_total.div_ceil(WORKGROUP_SIZE), 1, 1);
        span.end(&mut pass);
    }

    total_span.end(&mut pass);
    ready.set(particle_buffers.space);
}
