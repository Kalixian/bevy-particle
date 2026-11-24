//! N-body gravity between particles.
//!
//! Each frame, for every space with `Gravity`:
//! 1. `compact` (in `particle_sim.wesl`) packs every live particle whose emitter has
//!    `gravity: true` into `bodies` as `(x, y, mass, slot)` and counts them in `body_count`.
//! 2. `size_gravity` (in `gravity_args.wesl`) sizes the dispatches of step 3 from that count.
//! 3. One of:
//!    - brute force (`gravity_brute.wesl`): exact, O(n²);
//!    - Barnes-Hut (`gravity_bh.wesl`): bounds → Morton codes → radix sort → tree build →
//!      mass summary → tree walk, O(n log n).
//!
//!    Both write `accel[slot]`.
//! 4. `integrate` adds `accel[slot]` to those particles' velocity.

use std::borrow::Cow;

use bevy::{
    asset::load_embedded_asset,
    prelude::*,
    render::{
        render_resource::{
            binding_types::{storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer},
            *,
        },
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics},
        renderer::{RenderDevice, RenderQueue},
        RenderStartup,
    },
};

use crate::{gravity::GravityMethod, space::SimParams};

pub(super) const WORKGROUP_SIZE: u32 = 256;
/// Most storage buffers any Barnes-Hut layout binds (WebGPU's default limit per stage).
const BARNES_HUT_STORAGE_BUFFERS: u32 = 8;
/// Byte offsets into `GravityBuffers::args`. Must match `gravity_args.wesl`.
const BODY_ARGS: u64 = 0;
const HISTOGRAM_ARGS: u64 = 16;
const RADIX_PASSES: usize = 8;

pub(super) fn build(render_app: &mut SubApp) {
    render_app.add_systems(RenderStartup, init_pipelines);
}

/// Must match `struct RadixPass` in `gravity_bh.wesl`.
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub(super) struct RadixPassParams {
    shift: u32,
    src: u32,
    dst: u32,
}

/// One space's GPU-only working buffers, sized from its maximum body count `M`. They exist
/// only while the space has `Gravity`.
pub(super) struct GravityBuffers {
    /// `M` × `vec4<f32>`: (x, y, mass, bitcast slot index).
    pub bodies: Buffer,
    /// One `u32`: how many `bodies` were written this frame. Reset to 0 every frame.
    pub body_count: Buffer,
    /// `capacity` × `vec2<f32>`: acceleration per particle slot, read by `integrate`.
    pub accel: Buffer,
    /// Workgroup counts for the gravity passes.
    args: Buffer,
    /// Only while the space uses Barnes-Hut.
    barnes_hut: Option<BarnesHutBuffers>,
}

struct BarnesHutBuffers {
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
}

fn create_buffer(device: &RenderDevice, label: &'static str, size: u64) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl GravityBuffers {
    /// Keeps `buffers` in step with a space's settings: creates what they need and drops what
    /// they don't.
    pub fn update(
        buffers: &mut Option<Self>,
        params: &SimParams,
        pipelines: &GravityPipelines,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) {
        if !params.uses_gravity() {
            *buffers = None;
            return;
        }
        let max_bodies = params.gravity_max_bodies;
        let m = max_bodies as u64;
        let buffers = buffers.get_or_insert_with(|| Self {
            bodies: create_buffer(device, "gravity_bodies", m * 16),
            body_count: create_buffer(device, "gravity_body_count", 4),
            accel: create_buffer(device, "gravity_accel", params.capacity as u64 * 8),
            args: device.create_buffer(&BufferDescriptor {
                label: Some("gravity_args"),
                size: 32,
                usage: BufferUsages::STORAGE | BufferUsages::INDIRECT,
                mapped_at_creation: false,
            }),
            barnes_hut: None,
        });
        // Queue writes land before this frame's command buffers, so `compact` starts from 0.
        queue.write_buffer(&buffers.body_count, 0, &0u32.to_le_bytes());

        if !pipelines.uses_barnes_hut(params) {
            buffers.barnes_hut = None;
        } else if buffers.barnes_hut.is_none() {
            buffers.barnes_hut = Some(BarnesHutBuffers::new(max_bodies, device, queue));
        }
    }
}

impl BarnesHutBuffers {
    fn new(max_bodies: u32, device: &RenderDevice, queue: &RenderQueue) -> Self {
        let create = |label, size| create_buffer(device, label, size);
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
        radix_params.write_buffer(device, queue);

        Self {
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
        }
    }
}

#[derive(Resource)]
pub(super) struct GravityPipelines {
    args_layout: BindGroupLayoutDescriptor,
    size_gravity: CachedComputePipelineId,
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

pub(super) struct GravityBindGroups {
    args: BindGroup,
    method: MethodBindGroups,
}

/// For the method the space uses.
enum MethodBindGroups {
    BruteForce(BindGroup),
    BarnesHut(BarnesHutBindGroups),
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
        shader: load_embedded_asset!(asset_server.as_ref(), "shaders/gravity_brute.wesl"),
        entry_point: Some(Cow::from("gravity")),
        ..default()
    });

    let args_layout = BindGroupLayoutDescriptor::new(
        "gravity_args_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SimParams>(false),
                // body_count
                storage_buffer_read_only_sized(false, None),
                // gravity_args
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let size_gravity = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gravity_args".into()),
        layout: vec![args_layout.clone()],
        shader: load_embedded_asset!(asset_server.as_ref(), "shaders/gravity_args.wesl"),
        entry_point: Some(Cow::from("size_gravity")),
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
        args_layout,
        size_gravity,
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
    let shader: Handle<Shader> = load_embedded_asset!(asset_server, "shaders/gravity_bh.wesl");
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

impl GravityBindGroups {
    /// `None` if the buffers aren't ready.
    pub fn new(
        params: &UniformBuffer<SimParams>,
        buffers: &GravityBuffers,
        pipelines: &GravityPipelines,
        cache: &PipelineCache,
        device: &RenderDevice,
    ) -> Option<Self> {
        let params = params.binding()?;
        let accel = buffers.accel.as_entire_binding();
        let args = device.create_bind_group(
            "gravity_args_bind_group",
            &cache.get_bind_group_layout(&pipelines.args_layout),
            &BindGroupEntries::sequential((
                params.clone(),
                buffers.body_count.as_entire_binding(),
                buffers.args.as_entire_binding(),
            )),
        );
        let (Some(bh), Some(work)) = (&pipelines.barnes_hut, &buffers.barnes_hut) else {
            let method = MethodBindGroups::BruteForce(device.create_bind_group(
                "gravity_brute_bind_group",
                &cache.get_bind_group_layout(&pipelines.brute_layout),
                &BindGroupEntries::sequential((
                    params,
                    buffers.bodies.as_entire_binding(),
                    buffers.body_count.as_entire_binding(),
                    accel,
                )),
            ));
            return Some(Self { args, method });
        };
        let method = MethodBindGroups::BarnesHut(BarnesHutBindGroups {
            sort: device.create_bind_group(
                "gravity_bh_sort_bind_group",
                &cache.get_bind_group_layout(&bh.sort_layout),
                &BindGroupEntries::with_indices((
                    (0, params.clone()),
                    (1, work.radix_params.binding()?),
                    (2, buffers.bodies.as_entire_binding()),
                    (3, buffers.body_count.as_entire_binding()),
                    (5, work.bounds.as_entire_binding()),
                    (6, work.keys.as_entire_binding()),
                    (7, work.vals.as_entire_binding()),
                    (8, work.hist.as_entire_binding()),
                )),
            ),
            build: device.create_bind_group(
                "gravity_bh_build_bind_group",
                &cache.get_bind_group_layout(&bh.build_layout),
                &BindGroupEntries::with_indices((
                    (0, params.clone()),
                    (2, buffers.bodies.as_entire_binding()),
                    (3, buffers.body_count.as_entire_binding()),
                    (5, work.bounds.as_entire_binding()),
                    (6, work.keys.as_entire_binding()),
                    (7, work.vals.as_entire_binding()),
                    (9, work.nodes.as_entire_binding()),
                    (10, work.leaf_parent.as_entire_binding()),
                    (14, work.sorted_bodies.as_entire_binding()),
                )),
            ),
            walk: device.create_bind_group(
                "gravity_bh_walk_bind_group",
                &cache.get_bind_group_layout(&bh.walk_layout),
                &BindGroupEntries::with_indices((
                    (0, params),
                    (3, buffers.body_count.as_entire_binding()),
                    (4, accel),
                    (9, work.nodes.as_entire_binding()),
                    (10, work.leaf_parent.as_entire_binding()),
                    (11, work.node_data.as_entire_binding()),
                    (12, work.visits.as_entire_binding()),
                    (13, work.packed_nodes.as_entire_binding()),
                    (14, work.sorted_bodies.as_entire_binding()),
                )),
            ),
        });
        Some(Self { args, method })
    }
}

/// Which gravity passes a space runs this frame, with everything they need.
pub(super) struct GravityPlan<'a> {
    size_gravity: &'a ComputePipeline,
    args_bind_group: &'a BindGroup,
    args: &'a Buffer,
    method: MethodPlan<'a>,
}

enum MethodPlan<'a> {
    BruteForce { pipeline: &'a ComputePipeline, bind_group: &'a BindGroup },
    BarnesHut {
        pipelines: BarnesHutResolved<'a>,
        bind_groups: &'a BarnesHutBindGroups,
        buffers: &'a BarnesHutBuffers,
    },
}

struct BarnesHutResolved<'a> {
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
    fn uses_barnes_hut(&self, params: &SimParams) -> bool {
        params.gravity_method == GravityMethod::BarnesHut.as_gpu() && self.barnes_hut.is_some()
    }

    /// Resolves the passes a space needs. `None` while any of them is still compiling.
    pub(super) fn plan<'a>(
        &self,
        cache: &'a PipelineCache,
        buffers: &'a GravityBuffers,
        bind_groups: &'a GravityBindGroups,
    ) -> Option<GravityPlan<'a>> {
        let size_gravity = cache.get_compute_pipeline(self.size_gravity)?;
        let method = match &bind_groups.method {
            MethodBindGroups::BruteForce(bind_group) => {
                let pipeline = cache.get_compute_pipeline(self.brute_force)?;
                MethodPlan::BruteForce { pipeline, bind_group }
            }
            MethodBindGroups::BarnesHut(bind_groups) => MethodPlan::BarnesHut {
                pipelines: self.resolve_barnes_hut(cache)?,
                bind_groups,
                buffers: buffers.barnes_hut.as_ref()?,
            },
        };
        Some(GravityPlan {
            size_gravity,
            args_bind_group: &bind_groups.args,
            args: &buffers.args,
            method,
        })
    }

    fn resolve_barnes_hut<'a>(&self, cache: &'a PipelineCache) -> Option<BarnesHutResolved<'a>> {
        let bh = self.barnes_hut.as_ref()?;
        let get = |id| cache.get_compute_pipeline(id);
        Some(BarnesHutResolved {
            bounds: get(bh.bounds)?,
            morton: get(bh.morton)?,
            histogram: get(bh.histogram)?,
            scan_blocks: get(bh.scan_blocks)?,
            scan_sums: get(bh.scan_sums)?,
            scatter: get(bh.scatter)?,
            build_tree: get(bh.build_tree)?,
            summarize: get(bh.summarize)?,
            force: get(bh.force)?,
        })
    }
}

/// Clears the per-frame Barnes-Hut accumulators. Must be recorded before the compute pass.
pub(super) fn clear_buffers(encoder: &mut CommandEncoder, plan: &GravityPlan) {
    if let MethodPlan::BarnesHut { buffers, .. } = &plan.method {
        encoder.clear_buffer(&buffers.bounds, 0, None);
        encoder.clear_buffer(&buffers.visits, 0, None);
    }
}

/// Records the gravity passes. Runs after `compact` and before `integrate`.
pub(super) fn dispatch(
    pass: &mut ComputePass,
    plan: &GravityPlan,
    diagnostics: Option<&DiagnosticsRecorder>,
) {
    let args = plan.args;
    pass.set_bind_group(0, plan.args_bind_group, &[]);
    pass.set_pipeline(plan.size_gravity);
    pass.dispatch_workgroups(1, 1, 1);

    match &plan.method {
        MethodPlan::BruteForce { pipeline, bind_group } => {
            let span = diagnostics.time_span(pass, "gravity_brute_force");
            pass.set_bind_group(0, *bind_group, &[]);
            pass.set_pipeline(pipeline);
            pass.dispatch_workgroups_indirect(args, BODY_ARGS);
            span.end(pass);
        }
        MethodPlan::BarnesHut { pipelines: p, bind_groups: groups, buffers } => {
            let run_for = |pass: &mut ComputePass, pipeline: &ComputePipeline, offset: u64| {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups_indirect(args, offset);
            };
            // One thread per body.
            let run = |pass: &mut ComputePass, pipeline: &ComputePipeline| {
                run_for(pass, pipeline, BODY_ARGS);
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
                run(pass, p.bounds);
                run(pass, p.morton);
            });
            timed(pass, "gravity_bh_2_sort", &|pass| {
                for radix_pass in 0..RADIX_PASSES {
                    bind_sort(pass, radix_pass);
                    run(pass, p.histogram);
                    run_for(pass, p.scan_blocks, HISTOGRAM_ARGS);
                    pass.set_pipeline(p.scan_sums);
                    pass.dispatch_workgroups(1, 1, 1);
                    run(pass, p.scatter);
                }
            });
            timed(pass, "gravity_bh_3_build", &|pass| {
                pass.set_bind_group(0, &groups.build, &[]);
                run(pass, p.build_tree);
            });
            timed(pass, "gravity_bh_4_summarize", &|pass| {
                pass.set_bind_group(0, &groups.walk, &[]);
                run(pass, p.summarize);
            });
            timed(pass, "gravity_bh_5_force", &|pass| {
                pass.set_bind_group(0, &groups.walk, &[]);
                run(pass, p.force);
            });
        }
    }
}
