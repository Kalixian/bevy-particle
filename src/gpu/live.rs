//! Each space's list of live slots and the indirect draw arguments that go with it, rebuilt
//! after every simulation step by `live.wesl`.

use std::borrow::Cow;

use bevy::{
    asset::load_embedded_asset,
    prelude::*,
    render::{
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics},
        render_resource::{
            binding_types::{storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer},
            *,
        },
        renderer::RenderDevice,
        RenderStartup,
    },
};

use crate::space::SimParams;

pub(super) const WORKGROUP_SIZE: u32 = 256;

pub(super) fn build(render_app: &mut SubApp) {
    render_app.add_systems(RenderStartup, init_pipelines);
}

/// One space's buffers.
pub(super) struct LiveBuffers {
    /// One bit per slot.
    masks: Buffer,
    counts: Buffer,
    /// Live slot indices, in order. Only as many as `draw_args` says are valid.
    pub list: Buffer,
    /// For an indirect draw of every live particle. Zeroed (draws nothing) at first.
    pub draw_args: Buffer,
}

impl LiveBuffers {
    pub fn new(capacity: u32, device: &RenderDevice) -> Self {
        let create = |label, words: u32, usage| {
            device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: words as u64 * 4,
                usage: BufferUsages::STORAGE | usage,
                mapped_at_creation: false,
            })
        };
        let chunks = capacity.div_ceil(WORKGROUP_SIZE);
        let blocks = chunks.div_ceil(WORKGROUP_SIZE);
        Self {
            masks: create("particle_live_masks", chunks * 8, BufferUsages::empty()),
            counts: create("particle_live_counts", chunks + blocks, BufferUsages::empty()),
            list: create("particle_live_list", capacity, BufferUsages::empty()),
            draw_args: create("particle_draw_args", 4, BufferUsages::INDIRECT),
        }
    }
}

#[derive(Resource)]
pub(super) struct LivePipelines {
    layout: BindGroupLayoutDescriptor,
    mark: CachedComputePipelineId,
    scan_blocks: CachedComputePipelineId,
    scan_sums: CachedComputePipelineId,
    write: CachedComputePipelineId,
}

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "particle_live_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SimParams>(false),
                // particles
                storage_buffer_read_only_sized(false, None),
                // masks
                storage_buffer_sized(false, None),
                // counts
                storage_buffer_sized(false, None),
                // live
                storage_buffer_sized(false, None),
                // draw_args
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let shader: Handle<Shader> =
        load_embedded_asset!(asset_server.as_ref(), "shaders/live.wesl");
    let queue = |entry_point: &'static str| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("particle_live_{entry_point}").into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(Cow::from(entry_point)),
            ..default()
        })
    };
    let (mark, scan_blocks) = (queue("mark"), queue("scan_blocks"));
    let (scan_sums, write) = (queue("scan_sums"), queue("write"));
    commands.insert_resource(LivePipelines { layout, mark, scan_blocks, scan_sums, write });
}

impl LivePipelines {
    pub fn bind_group(
        &self,
        params: BindingResource,
        particles: &Buffer,
        buffers: &LiveBuffers,
        cache: &PipelineCache,
        device: &RenderDevice,
    ) -> BindGroup {
        device.create_bind_group(
            "particle_live_bind_group",
            &cache.get_bind_group_layout(&self.layout),
            &BindGroupEntries::sequential((
                params,
                particles.as_entire_binding(),
                buffers.masks.as_entire_binding(),
                buffers.counts.as_entire_binding(),
                buffers.list.as_entire_binding(),
                buffers.draw_args.as_entire_binding(),
            )),
        )
    }

    /// `None` while any pipeline is still compiling.
    pub fn resolve<'a>(&self, cache: &'a PipelineCache) -> Option<LivePasses<'a>> {
        let get = |id| cache.get_compute_pipeline(id);
        Some(LivePasses {
            mark: get(self.mark)?,
            scan_blocks: get(self.scan_blocks)?,
            scan_sums: get(self.scan_sums)?,
            write: get(self.write)?,
        })
    }
}

pub(super) struct LivePasses<'a> {
    mark: &'a ComputePipeline,
    scan_blocks: &'a ComputePipeline,
    scan_sums: &'a ComputePipeline,
    write: &'a ComputePipeline,
}

impl LivePasses<'_> {
    pub fn dispatch(
        &self,
        pass: &mut ComputePass,
        bind_group: &BindGroup,
        capacity: u32,
        diagnostics: Option<&DiagnosticsRecorder>,
    ) {
        let chunks = capacity.div_ceil(WORKGROUP_SIZE);
        let span = diagnostics.time_span(pass, "live");
        pass.set_bind_group(0, bind_group, &[]);
        for (pipeline, workgroups) in [
            (self.mark, chunks),
            (self.scan_blocks, chunks.div_ceil(WORKGROUP_SIZE)),
            (self.scan_sums, 1),
            (self.write, chunks),
        ] {
            pass.set_pipeline(pipeline);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        span.end(pass);
    }
}
