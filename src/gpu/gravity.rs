//! N-body gravity between particles.
//!
//! Each frame, when the space has `Gravity`:
//! 1. `compact` (in `particle_sim.wgsl`) packs every live particle whose emitter has
//!    `gravity: true` into `bodies` as `(x, y, mass, slot)` and counts them in `body_count`.
//! 2. One of:
//!    - brute force (`gravity_brute.wgsl`): exact, O(n²);
//!    - Barnes-Hut (`gravity_bh.wgsl`): bounds → Morton codes → radix sort → tree build →
//!      mass summary → tree walk, O(n log n).
//!
//!    Both write `accel[slot]`.
//! 3. `integrate` adds `accel[slot]` to those particles' velocity.

use std::borrow::Cow;

use bevy::{
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer},
            *,
        },
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics},
        renderer::{RenderDevice, RenderQueue},
        storage::GpuShaderBuffer,
        Render, RenderStartup, RenderSystems,
    },
};

use super::compute::SimBuffers;
use crate::{particle::ParticleBuffers, space::SimParams};

const BRUTE_SHADER: &str = "shaders/gravity_brute.wgsl";
const BARNES_HUT_SHADER: &str = "shaders/gravity_bh.wgsl";
const WORKGROUP_SIZE: u32 = 256;
/// Most storage buffers any Barnes-Hut layout binds (WebGPU's default limit per stage).
const BARNES_HUT_STORAGE_BUFFERS: u32 = 8;
const RADIX_PASSES: usize = 8;
/// Must match `FORCE_WG` in `gravity_bh.wgsl`.
const FORCE_WORKGROUP_SIZE: u32 = 256;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(
            Render,
            (
                prepare_gravity_buffers.in_set(RenderSystems::PrepareResources),
                prepare_bind_groups.in_set(RenderSystems::PrepareBindGroups),
            ),
        );
}

/// Must match `struct RadixPass` in `gravity_bh.wgsl`.
#[derive(ShaderType, Clone, Copy, Default)]
struct RadixPassParams {
    shift: u32,
    src: u32,
    dst: u32,
}

/// GPU-only working buffers, sized from the maximum body count `M`.
#[derive(Resource)]
pub(super) struct GravityBuffers {
    /// `M` × `vec4<f32>`: (x, y, mass, bitcast slot index).
    pub bodies: Buffer,
    /// One `u32`: how many `bodies` were written this frame. Reset to 0 every frame.
    pub body_count: Buffer,
    // Barnes-Hut only:
    bounds: Buffer,
    keys: Buffer,
    vals: Buffer,
    hist: Buffer,
    nodes: Buffer,
    leaf_parent: Buffer,
    node_data: Buffer,
    visits: Buffer,
    packed_nodes: Buffer,
    sorted_bodies: Buffer,
    radix_params: DynamicUniformBuffer<RadixPassParams>,
    radix_offsets: [u32; RADIX_PASSES],
    max_bodies: u32,
}

#[derive(Resource)]
pub(super) struct GravityPipelines {
    brute_layout: BindGroupLayoutDescriptor,
    brute_force: CachedComputePipelineId,
    /// `None` when the GPU can't bind enough storage buffers; brute force is used instead.
    barnes_hut: Option<BarnesHutPipelines>,
}

struct BarnesHutPipelines {
    /// Bounds, Morton codes and radix sort.
    sort_layout: BindGroupLayoutDescriptor,
    build_layout: BindGroupLayoutDescriptor,
    /// Mass summary and force.
    walk_layout: BindGroupLayoutDescriptor,
    bounds: CachedComputePipelineId,
    morton: CachedComputePipelineId,
    histogram: CachedComputePipelineId,
    scan_blocks: CachedComputePipelineId,
    scan_sums: CachedComputePipelineId,
    scatter: CachedComputePipelineId,
    build_tree: CachedComputePipelineId,
    summarize: CachedComputePipelineId,
    force: CachedComputePipelineId,
}

#[derive(Resource)]
pub(super) struct GravityBindGroups {
    brute: BindGroup,
    barnes_hut: Option<BarnesHutBindGroups>,
}

struct BarnesHutBindGroups {
    sort: BindGroup,
    build: BindGroup,
    walk: BindGroup,
}

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
) {
    let brute_layout = BindGroupLayoutDescriptor::new(
        "gravity_brute_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SimParams>(false),
                // bodies
                storage_buffer_read_only_sized(false, None),
                // body_count
                storage_buffer_read_only_sized(false, None),
                // accel
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let brute_force = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gravity_brute".into()),
        layout: vec![brute_layout.clone()],
        shader: asset_server.load(BRUTE_SHADER),
        entry_point: Some(Cow::from("gravity")),
        ..default()
    });

    let storage_limit = device.limits().max_storage_buffers_per_shader_stage;
    let barnes_hut = if storage_limit >= BARNES_HUT_STORAGE_BUFFERS {
        Some(queue_barnes_hut(&asset_server, &pipeline_cache))
    } else {
        warn!(
            "This GPU allows {storage_limit} storage buffers per shader stage; Barnes-Hut needs \
             {BARNES_HUT_STORAGE_BUFFERS}. Gravity will use brute force."
        );
        None
    };

    commands.insert_resource(GravityPipelines {
        brute_layout,
        brute_force,
        barnes_hut,
    });
}

fn queue_barnes_hut(asset_server: &AssetServer, pipeline_cache: &PipelineCache) -> BarnesHutPipelines {
    let storage = || storage_buffer_sized(false, None);
    // Each layout binds only what its entry points use, to stay within
    // BARNES_HUT_STORAGE_BUFFERS. Numbers are the `@binding`s in the shader.
    let sort_layout = BindGroupLayoutDescriptor::new(
        "gravity_bh_sort_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<SimParams>(false)),
                (1, uniform_buffer::<RadixPassParams>(true)),
                (2, storage()), // bodies
                (3, storage()), // body_count
                (5, storage()), // bounds
                (6, storage()), // keys
                (7, storage()), // vals
                (8, storage()), // hist
            ),
        ),
    );
    let build_layout = BindGroupLayoutDescriptor::new(
        "gravity_bh_build_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<SimParams>(false)),
                (2, storage()),  // bodies
                (3, storage()),  // body_count
                (5, storage()),  // bounds
                (6, storage()),  // keys
                (7, storage()),  // vals
                (9, storage()),  // nodes
                (10, storage()), // leaf_parent
                (14, storage()), // sorted_bodies
            ),
        ),
    );
    let walk_layout = BindGroupLayoutDescriptor::new(
        "gravity_bh_walk_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<SimParams>(false)),
                (3, storage()),  // body_count
                (4, storage()),  // accel
                (9, storage()),  // nodes
                (10, storage()), // leaf_parent
                (11, storage()), // node_data
                (12, storage()), // visits
                (13, storage()), // packed_nodes
                (14, storage()), // sorted_bodies
            ),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(BARNES_HUT_SHADER);
    let queue = |entry_point: &'static str, layout: &BindGroupLayoutDescriptor| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("gravity_bh_{entry_point}").into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(Cow::from(entry_point)),
            ..default()
        })
    };
    BarnesHutPipelines {
        bounds: queue("bounds_pass", &sort_layout),
        morton: queue("morton", &sort_layout),
        histogram: queue("radix_histogram", &sort_layout),
        scan_blocks: queue("radix_scan_blocks", &sort_layout),
        scan_sums: queue("radix_scan_sums", &sort_layout),
        scatter: queue("radix_scatter", &sort_layout),
        build_tree: queue("build_tree", &build_layout),
        summarize: queue("summarize", &walk_layout),
        force: queue("force", &walk_layout),
        sort_layout,
        build_layout,
        walk_layout,
    }
}

fn prepare_gravity_buffers(
    mut commands: Commands,
    params: Res<SimParams>,
    existing: Option<Res<GravityBuffers>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let max_bodies = params.gravity_max_bodies;
    if max_bodies == 0 {
        return;
    }

    if let Some(buffers) = existing.filter(|b| b.max_bodies == max_bodies) {
        // Queue writes land before this frame's command buffers, so `compact` starts from 0.
        queue.write_buffer(&buffers.body_count, 0, &0u32.to_le_bytes());
        return;
    }

    // New buffers are zeroed by wgpu, so the count already starts at 0.
    let create = |label: &'static str, size: u64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size.max(16),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let m = max_bodies as u64;
    let tiles = m.div_ceil(WORKGROUP_SIZE as u64);

    // Radix pass p sorts bits 4p..4p+4, from one half of keys/vals into the other.
    let mut radix_params = DynamicUniformBuffer::default();
    let radix_offsets = std::array::from_fn(|p| {
        let (src, dst) = if p % 2 == 0 { (0, max_bodies) } else { (max_bodies, 0) };
        radix_params.push(&RadixPassParams {
            shift: 4 * p as u32,
            src,
            dst,
        })
    });
    radix_params.write_buffer(&device, &queue);

    commands.insert_resource(GravityBuffers {
        bodies: create("gravity_bodies", m * 16),
        body_count: create("gravity_body_count", 4),
        bounds: create("gravity_bounds", 16),
        keys: create("gravity_keys", 2 * m * 4),
        vals: create("gravity_vals", 2 * m * 4),
        // 16 digits × tiles, plus one sum per 256-entry block (at most 256).
        hist: create("gravity_hist", (16 * tiles + 256) * 4),
        nodes: create("gravity_nodes", m * 16),
        leaf_parent: create("gravity_leaf_parent", m * 4),
        node_data: create("gravity_node_data", 3 * m * 4),
        visits: create("gravity_visits", m * 4),
        packed_nodes: create("gravity_packed_nodes", m * 16),
        sorted_bodies: create("gravity_sorted_bodies", m * 16),
        radix_params,
        radix_offsets,
        max_bodies,
    });
}

fn prepare_bind_groups(
    mut commands: Commands,
    pipelines: Res<GravityPipelines>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    sim_buffers: Res<SimBuffers>,
    buffers: Option<Res<GravityBuffers>>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    let accel = particle_buffers.and_then(|b| gpu_buffers.get(&b.accel));
    let (Some(buffers), Some(accel), Some(radix_params)) = (
        buffers.as_deref(),
        accel,
        buffers.as_ref().and_then(|b| b.radix_params.binding()),
    ) else {
        commands.remove_resource::<GravityBindGroups>();
        return;
    };
    let accel = accel.buffer.as_entire_binding();

    let brute = device.create_bind_group(
        "gravity_brute_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.brute_layout),
        &BindGroupEntries::sequential((
            &sim_buffers.params,
            buffers.bodies.as_entire_binding(),
            buffers.body_count.as_entire_binding(),
            accel.clone(),
        )),
    );
    let barnes_hut = pipelines.barnes_hut.as_ref().map(|bh| BarnesHutBindGroups {
        sort: device.create_bind_group(
            "gravity_bh_sort_bind_group",
            &pipeline_cache.get_bind_group_layout(&bh.sort_layout),
            &BindGroupEntries::with_indices((
                (0, &sim_buffers.params),
                (1, radix_params),
                (2, buffers.bodies.as_entire_binding()),
                (3, buffers.body_count.as_entire_binding()),
                (5, buffers.bounds.as_entire_binding()),
                (6, buffers.keys.as_entire_binding()),
                (7, buffers.vals.as_entire_binding()),
                (8, buffers.hist.as_entire_binding()),
            )),
        ),
        build: device.create_bind_group(
            "gravity_bh_build_bind_group",
            &pipeline_cache.get_bind_group_layout(&bh.build_layout),
            &BindGroupEntries::with_indices((
                (0, &sim_buffers.params),
                (2, buffers.bodies.as_entire_binding()),
                (3, buffers.body_count.as_entire_binding()),
                (5, buffers.bounds.as_entire_binding()),
                (6, buffers.keys.as_entire_binding()),
                (7, buffers.vals.as_entire_binding()),
                (9, buffers.nodes.as_entire_binding()),
                (10, buffers.leaf_parent.as_entire_binding()),
                (14, buffers.sorted_bodies.as_entire_binding()),
            )),
        ),
        walk: device.create_bind_group(
            "gravity_bh_walk_bind_group",
            &pipeline_cache.get_bind_group_layout(&bh.walk_layout),
            &BindGroupEntries::with_indices((
                (0, &sim_buffers.params),
                (3, buffers.body_count.as_entire_binding()),
                (4, accel),
                (9, buffers.nodes.as_entire_binding()),
                (10, buffers.leaf_parent.as_entire_binding()),
                (11, buffers.node_data.as_entire_binding()),
                (12, buffers.visits.as_entire_binding()),
                (13, buffers.packed_nodes.as_entire_binding()),
                (14, buffers.sorted_bodies.as_entire_binding()),
            )),
        ),
    });
    commands.insert_resource(GravityBindGroups { brute, barnes_hut });
}

/// Which gravity passes run this frame, with their pipelines resolved.
pub(super) enum GravityPlan<'a> {
    BruteForce(&'a ComputePipeline),
    BarnesHut(BarnesHutResolved<'a>),
}

pub(super) struct BarnesHutResolved<'a> {
    bounds: &'a ComputePipeline,
    morton: &'a ComputePipeline,
    histogram: &'a ComputePipeline,
    scan_blocks: &'a ComputePipeline,
    scan_sums: &'a ComputePipeline,
    scatter: &'a ComputePipeline,
    build_tree: &'a ComputePipeline,
    summarize: &'a ComputePipeline,
    force: &'a ComputePipeline,
}

impl GravityPipelines {
    /// Resolves the passes this frame needs. `None` while any of them is still compiling.
    pub(super) fn plan<'a>(
        &self,
        cache: &'a PipelineCache,
        params: &SimParams,
        bind_groups: &GravityBindGroups,
    ) -> Option<GravityPlan<'a>> {
        let use_barnes_hut = params.gravity_method == 1 && bind_groups.barnes_hut.is_some();
        let Some(bh) = self.barnes_hut.as_ref().filter(|_| use_barnes_hut) else {
            return Some(GravityPlan::BruteForce(
                cache.get_compute_pipeline(self.brute_force)?,
            ));
        };
        let get = |id| cache.get_compute_pipeline(id);
        Some(GravityPlan::BarnesHut(BarnesHutResolved {
            bounds: get(bh.bounds)?,
            morton: get(bh.morton)?,
            histogram: get(bh.histogram)?,
            scan_blocks: get(bh.scan_blocks)?,
            scan_sums: get(bh.scan_sums)?,
            scatter: get(bh.scatter)?,
            build_tree: get(bh.build_tree)?,
            summarize: get(bh.summarize)?,
            force: get(bh.force)?,
        }))
    }
}

/// Clears the per-frame Barnes-Hut accumulators. Must be recorded before the compute pass.
pub(super) fn clear_buffers(encoder: &mut CommandEncoder, plan: &GravityPlan, buffers: &GravityBuffers) {
    if let GravityPlan::BarnesHut(_) = plan {
        encoder.clear_buffer(&buffers.bounds, 0, None);
        encoder.clear_buffer(&buffers.visits, 0, None);
    }
}

/// Records the gravity passes. Runs after `compact` and before `integrate`.
pub(super) fn dispatch(
    pass: &mut ComputePass,
    plan: &GravityPlan,
    bind_groups: &GravityBindGroups,
    buffers: &GravityBuffers,
    diagnostics: Option<&DiagnosticsRecorder>,
) {
    // The body count is only known on the GPU, so every pass is sized for the maximum;
    // workgroups past the real count exit immediately.
    let body_groups = buffers.max_bodies.div_ceil(WORKGROUP_SIZE);

    match plan {
        GravityPlan::BruteForce(pipeline) => {
            let span = diagnostics.time_span(pass, "gravity_brute_force");
            pass.set_bind_group(0, &bind_groups.brute, &[]);
            pass.set_pipeline(pipeline);
            pass.dispatch_workgroups(body_groups, 1, 1);
            span.end(pass);
        }
        GravityPlan::BarnesHut(p) => {
            let Some(groups) = &bind_groups.barnes_hut else {
                return;
            };
            let scan_groups = (16 * body_groups).div_ceil(WORKGROUP_SIZE);
            let run = |pass: &mut ComputePass, pipeline: &ComputePipeline, workgroups: u32| {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups(workgroups, 1, 1);
            };
            // Only the sort layout has the per-radix-pass uniform (a dynamic offset).
            let bind_sort = |pass: &mut ComputePass, radix_pass: usize| {
                pass.set_bind_group(0, &groups.sort, &[buffers.radix_offsets[radix_pass]]);
            };
            let timed = |pass: &mut ComputePass, name: &'static str, body: &dyn Fn(&mut ComputePass)| {
                let span = diagnostics.time_span(pass, name);
                body(pass);
                span.end(pass);
            };

            timed(pass, "gravity_bh_1_bounds_morton", &|pass| {
                bind_sort(pass, 0);
                run(pass, p.bounds, body_groups);
                run(pass, p.morton, body_groups);
            });
            timed(pass, "gravity_bh_2_sort", &|pass| {
                for radix_pass in 0..RADIX_PASSES {
                    bind_sort(pass, radix_pass);
                    run(pass, p.histogram, body_groups);
                    run(pass, p.scan_blocks, scan_groups);
                    run(pass, p.scan_sums, 1);
                    run(pass, p.scatter, body_groups);
                }
            });
            timed(pass, "gravity_bh_3_build", &|pass| {
                pass.set_bind_group(0, &groups.build, &[]);
                run(pass, p.build_tree, body_groups);
            });
            timed(pass, "gravity_bh_4_summarize", &|pass| {
                pass.set_bind_group(0, &groups.walk, &[]);
                run(pass, p.summarize, body_groups);
            });
            let force_groups = buffers.max_bodies.div_ceil(FORCE_WORKGROUP_SIZE);
            timed(pass, "gravity_bh_5_force", &|pass| {
                pass.set_bind_group(0, &groups.walk, &[]);
                run(pass, p.force, force_groups);
            });
        }
    }
}
