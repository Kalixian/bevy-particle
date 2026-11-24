use bevy::{
    asset::load_embedded_asset,
    core_pipeline::{core_2d::main_transparent_pass_2d, schedule::Core2d, Core2dSystems},
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{storage_buffer_read_only_sized, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        storage::GpuShaderBuffer,
        view::{ExtractedView, Msaa, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderStartup, RenderSystems,
    },
};

use crate::space::SpaceUploads;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SpecializedRenderPipelines<DrawPipeline>>()
        .init_resource::<DrawBindGroups>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (
                specialize_views.in_set(RenderSystems::Queue),
                prepare_bind_groups.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        // After sprites/meshes, before bloom and tonemapping (Core2dSystems::PostProcess),
        // so particles are drawn into the HDR target and bloom picks them up.
        .add_systems(
            Core2d,
            draw_particles
                .after(main_transparent_pass_2d)
                .in_set(Core2dSystems::MainPass),
        );
}

#[derive(Resource)]
struct DrawPipeline {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct DrawPipelineKey {
    format: TextureFormat,
    samples: u32,
}

impl SpecializedRenderPipeline for DrawPipeline {
    type Key = DrawPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("particle_draw".into()),
            layout: vec![self.layout.clone()],
            // No vertex buffers: the shader builds each quad from vertex_index and reads the
            // particle from the storage buffer with instance_index.
            vertex: VertexState {
                shader: self.shader.clone(),
                entry_point: Some("vertex".into()),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: key.format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            multisample: MultisampleState {
                count: key.samples,
                ..default()
            },
            ..default()
        }
    }
}

/// The specialized pipeline for one view.
#[derive(Component)]
struct ViewDrawPipeline(CachedRenderPipelineId);

/// Each space's bind group and capacity, in draw order.
#[derive(Resource, Default)]
struct DrawBindGroups(Vec<(BindGroup, u32)>);

fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    let layout = BindGroupLayoutDescriptor::new(
        "particle_draw_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX,
            (
                uniform_buffer::<ViewUniform>(true),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    );
    commands.insert_resource(DrawPipeline {
        layout,
        shader: load_embedded_asset!(asset_server.as_ref(), "shaders/particle_draw.wesl"),
    });
}

fn specialize_views(
    mut commands: Commands,
    pipeline: Res<DrawPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<DrawPipeline>>,
    views: Query<(Entity, &ViewTarget, &Msaa), With<ExtractedView>>,
) {
    for (entity, target, msaa) in &views {
        let key = DrawPipelineKey {
            format: target.main_texture_format(),
            samples: msaa.samples(),
        };
        let id = pipelines.specialize(&pipeline_cache, &pipeline, key);
        commands.entity(entity).insert(ViewDrawPipeline(id));
    }
}

fn prepare_bind_groups(
    pipeline: Res<DrawPipeline>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    view_uniforms: Res<ViewUniforms>,
    spaces: Res<SpaceUploads>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
    mut bind_groups: ResMut<DrawBindGroups>,
) {
    bind_groups.0.clear();
    let Some(view_binding) = view_uniforms.uniforms.binding() else {
        return;
    };
    let layout = pipeline_cache.get_bind_group_layout(&pipeline.layout);
    bind_groups.0.extend(spaces.0.iter().filter_map(|space| {
        let particles = gpu_buffers.get(&space.buffers.particles)?;
        let bind_group = device.create_bind_group(
            "particle_draw_bind_group",
            &layout,
            &BindGroupEntries::sequential((
                view_binding.clone(),
                particles.buffer.as_entire_binding(),
            )),
        );
        Some((bind_group, space.buffers.capacity))
    }));
}

fn draw_particles(
    view: ViewQuery<(&ViewTarget, &ViewUniformOffset, &ViewDrawPipeline)>,
    pipeline_cache: Res<PipelineCache>,
    bind_groups: Res<DrawBindGroups>,
    mut ctx: RenderContext,
) {
    let (target, view_offset, view_pipeline) = view.into_inner();
    let Some(render_pipeline) = pipeline_cache.get_render_pipeline(view_pipeline.0) else {
        return;
    };
    if bind_groups.0.is_empty() {
        return;
    }

    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("particle_draw"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(render_pipeline);
    for (bind_group, capacity) in &bind_groups.0 {
        pass.set_bind_group(0, bind_group, &[view_offset.offset]);
        pass.draw(0..6, 0..*capacity);
    }
}
