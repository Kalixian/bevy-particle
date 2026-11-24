use std::borrow::Cow;

use bevy::{
    asset::load_embedded_asset,
    platform::collections::{HashMap, HashSet},
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
        storage::{GpuShaderBuffer, ShaderBuffer},
        Render, RenderStartup, RenderSystems,
    },
};

use super::{
    gravity::{self, GravityBindGroups, GravityBuffers, GravityPipelines, GravityPlan},
    live::{LiveBuffers, LivePipelines},
    GpuSimReady,
};
use crate::{
    emitter::{EmitterUpload, GpuEmitter},
    modifier::{GpuModifier, ModifierUpload},
    spread::GpuCurve,
    track::GpuTrackRecord,
    space::{SimParams, SpaceUpload, SpaceUploads},
};

/// Must match `@workgroup_size` in the shaders.
pub(super) const WORKGROUP_SIZE: u32 = 256;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SimBuffers>()
        .init_resource::<SpaceGpus>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(
            Render,
            (
                (write_sim_buffers, prepare_spaces).in_set(RenderSystems::PrepareResources),
                prepare_bind_groups.in_set(RenderSystems::PrepareBindGroups),
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

/// Small per-frame buffers written from the CPU, shared by all spaces.
#[derive(Resource, Default)]
struct SimBuffers {
    emitters: StorageBuffer<Vec<GpuEmitter>>,
    modifiers: StorageBuffer<Vec<GpuModifier>>,
    curves: StorageBuffer<Vec<GpuCurve>>,
    tracks: StorageBuffer<Vec<GpuTrackRecord>>,
    /// Stands in for the acceleration buffer of spaces without gravity, which never read it.
    no_accel: StorageBuffer<Vec<Vec2>>,
}

/// Render-world state of each space, by [`ParticleBuffers::id`].
///
/// [`ParticleBuffers::id`]: crate::particle::ParticleBuffers::id
#[derive(Resource, Default)]
pub(super) struct SpaceGpus(pub HashMap<AssetId<ShaderBuffer>, SpaceGpu>);

pub(super) struct SpaceGpu {
    params: UniformBuffer<SimParams>,
    pub live: LiveBuffers,
    /// `Some` while the space has gravity.
    gravity: Option<GravityBuffers>,
    /// `None` until everything they bind is ready.
    bind_groups: Option<SpaceBindGroups>,
}

struct SpaceBindGroups {
    sim: BindGroup,
    live: BindGroup,
    /// For `compact`, and for the gravity passes.
    gravity: Option<(BindGroup, GravityBindGroups)>,
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
    emitters: Res<EmitterUpload>,
    modifiers: Res<ModifierUpload>,
    mut buffers: ResMut<SimBuffers>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    buffers.emitters.set(emitters.emitters.clone());
    buffers.emitters.write_buffer(&device, &queue);
    buffers.modifiers.set(modifiers.gpu_data());
    buffers.modifiers.write_buffer(&device, &queue);
    buffers.curves.set(emitters.curves_gpu_data());
    buffers.curves.write_buffer(&device, &queue);
    buffers.tracks.set(modifiers.tracks_gpu_data());
    buffers.tracks.write_buffer(&device, &queue);
    if buffers.no_accel.buffer().is_none() {
        buffers.no_accel.set(vec![Vec2::ZERO]);
        buffers.no_accel.write_buffer(&device, &queue);
    }
}

/// Drops the state of removed spaces and updates the buffers of the others.
fn prepare_spaces(
    uploads: Res<SpaceUploads>,
    mut spaces: ResMut<SpaceGpus>,
    gravity_pipelines: Res<GravityPipelines>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let current: HashSet<_> = uploads.0.iter().map(|upload| upload.buffers.id()).collect();
    spaces.0.retain(|id, _| current.contains(id));
    for SpaceUpload { buffers, params, .. } in &uploads.0 {
        let space = spaces.0.entry(buffers.id()).or_insert_with(|| SpaceGpu {
            params: default(),
            live: LiveBuffers::new(buffers.capacity, &device),
            gravity: None,
            bind_groups: None,
        });
        space.params.set(*params);
        space.params.write_buffer(&device, &queue);
        GravityBuffers::update(&mut space.gravity, params, &gravity_pipelines, &device, &queue);
    }
}

fn prepare_bind_groups(
    pipelines: Res<SimPipelines>,
    gravity_pipelines: Res<GravityPipelines>,
    live_pipelines: Res<LivePipelines>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    sim_buffers: Res<SimBuffers>,
    uploads: Res<SpaceUploads>,
    mut spaces: ResMut<SpaceGpus>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    for upload in &uploads.0 {
        let Some(space) = spaces.0.get_mut(&upload.buffers.id()) else {
            continue;
        };
        space.bind_groups = create_bind_groups(
            upload,
            space,
            &sim_buffers,
            &pipelines,
            &gravity_pipelines,
            &live_pipelines,
            &pipeline_cache,
            &device,
            &gpu_buffers,
        );
    }
}

fn create_bind_groups(
    upload: &SpaceUpload,
    space: &SpaceGpu,
    sim_buffers: &SimBuffers,
    pipelines: &SimPipelines,
    gravity_pipelines: &GravityPipelines,
    live_pipelines: &LivePipelines,
    pipeline_cache: &PipelineCache,
    device: &RenderDevice,
    gpu_buffers: &RenderAssets<GpuShaderBuffer>,
) -> Option<SpaceBindGroups> {
    let particles = gpu_buffers.get(&upload.buffers.particles)?;
    let free_list = gpu_buffers.get(&upload.buffers.free_list)?;
    let emitters = sim_buffers.emitters.binding()?;
    let accel = match &space.gravity {
        Some(gravity) => gravity.accel.as_entire_binding(),
        None => sim_buffers.no_accel.binding()?,
    };

    let sim = device.create_bind_group(
        "particle_sim_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::with_indices((
            (0, space.params.binding()?),
            (1, particles.buffer.as_entire_binding()),
            (2, free_list.buffer.as_entire_binding()),
            (3, emitters.clone()),
            (4, sim_buffers.modifiers.binding()?),
            (7, accel),
            (8, sim_buffers.curves.binding()?),
            (9, sim_buffers.tracks.binding()?),
        )),
    );
    let live = live_pipelines.bind_group(
        space.params.binding()?,
        &particles.buffer,
        &space.live,
        pipeline_cache,
        device,
    );
    let gravity = match &space.gravity {
        Some(gravity) => {
            let compact = device.create_bind_group(
                "particle_compact_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.compact_layout),
                &BindGroupEntries::with_indices((
                    (0, space.params.binding()?),
                    (1, particles.buffer.as_entire_binding()),
                    (3, emitters),
                    (5, gravity.bodies.as_entire_binding()),
                    (6, gravity.body_count.as_entire_binding()),
                )),
            );
            let bind_groups = GravityBindGroups::new(
                &space.params,
                gravity,
                gravity_pipelines,
                pipeline_cache,
                device,
            )?;
            Some((compact, bind_groups))
        }
        None => None,
    };
    Some(SpaceBindGroups { sim, live, gravity })
}

/// One space's passes this frame.
struct SpacePlan<'a> {
    params: &'a SimParams,
    sim: &'a BindGroup,
    /// `None` for hidden spaces, whose live list nothing needs.
    live: Option<&'a BindGroup>,
    gravity: Option<SpaceGravityPlan<'a>>,
}

struct SpaceGravityPlan<'a> {
    compact: &'a ComputePipeline,
    compact_bind_group: &'a BindGroup,
    gravity: GravityPlan<'a>,
}

fn plan_space<'a>(
    upload: &'a SpaceUpload,
    space: &'a SpaceGpu,
    pipelines: &SimPipelines,
    gravity_pipelines: &GravityPipelines,
    cache: &'a PipelineCache,
) -> Option<SpacePlan<'a>> {
    let bind_groups = space.bind_groups.as_ref()?;
    let gravity = match (&space.gravity, &bind_groups.gravity) {
        // Wait for this space's gravity pipelines too (Barnes-Hut compiles nine of them).
        (Some(buffers), Some((compact_bind_group, bind_groups))) => Some(SpaceGravityPlan {
            compact: cache.get_compute_pipeline(pipelines.compact)?,
            compact_bind_group: compact_bind_group,
            gravity: gravity_pipelines.plan(cache, buffers, bind_groups)?,
        }),
        _ => None,
    };
    Some(SpacePlan {
        params: &upload.params,
        sim: &bind_groups.sim,
        live: upload.visible.then_some(&bind_groups.live),
        gravity,
    })
}

fn dispatch_sim(
    mut ctx: RenderContext,
    pipelines: Res<SimPipelines>,
    gravity_pipelines: Res<GravityPipelines>,
    live_pipelines: Res<LivePipelines>,
    pipeline_cache: Res<PipelineCache>,
    uploads: Res<SpaceUploads>,
    spaces: Res<SpaceGpus>,
    ready: Res<GpuSimReady>,
) {
    let mut simulated = HashSet::default();
    let mut plans = Vec::new();
    let sim_pipelines = (|| {
        Some((
            pipeline_cache.get_compute_pipeline(pipelines.integrate)?,
            pipeline_cache.get_compute_pipeline(pipelines.spawn)?,
            live_pipelines.resolve(&pipeline_cache)?,
        ))
    })();
    if sim_pipelines.is_some() {
        for upload in &uploads.0 {
            let id = upload.buffers.id();
            let plan = spaces.0.get(&id).and_then(|space| {
                plan_space(upload, space, &pipelines, &gravity_pipelines, &pipeline_cache)
            });
            if let Some(plan) = plan {
                simulated.insert(id);
                plans.push(plan);
            }
        }
    }
    ready.set(simulated);
    let Some((integrate, spawn, live_passes)) = sim_pipelines else {
        return;
    };
    if plans.is_empty() {
        return;
    }

    // GPU timing spans ("render/particles/..."); no-ops unless RenderDiagnosticsPlugin is
    // added (the `profile` feature). With several spaces, each records its own.
    let recorder = ctx.diagnostic_recorder();
    let diagnostics = recorder.as_deref();

    let encoder = ctx.command_encoder();
    for gravity in plans.iter().filter_map(|plan| plan.gravity.as_ref()) {
        gravity::clear_buffers(encoder, &gravity.gravity);
    }

    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("particle_sim"),
        ..default()
    });
    let total_span = diagnostics.time_span(&mut pass, "particles");
    // Each dispatch is its own synchronization scope, so every pass sees the writes of the
    // passes before it.
    for SpacePlan { params, sim, live, gravity } in &plans {
        let slot_groups = params.capacity.div_ceil(WORKGROUP_SIZE);

        if let Some(plan) = gravity {
            let span = diagnostics.time_span(&mut pass, "compact");
            pass.set_bind_group(0, plan.compact_bind_group, &[]);
            pass.set_pipeline(plan.compact);
            pass.dispatch_workgroups(slot_groups, 1, 1);
            span.end(&mut pass);

            gravity::dispatch(&mut pass, &plan.gravity, diagnostics);
        }

        pass.set_bind_group(0, *sim, &[]);

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

        if let Some(live) = live {
            live_passes.dispatch(&mut pass, live, params.capacity, diagnostics);
        }
    }
    total_span.end(&mut pass);
}
