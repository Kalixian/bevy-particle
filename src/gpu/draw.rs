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

use crate::particle::ParticleBuffers;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SpecializedRenderPipelines<DrawPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (
                specialize_views.in_set(RenderSystems::Queue),
                prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
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

#[derive(Resource)]
struct DrawBindGroup(BindGroup);

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

fn prepare_bind_group(
    mut commands: Commands,
    pipeline: Res<DrawPipeline>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    view_uniforms: Res<ViewUniforms>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    let (Some(view_binding), Some(particles)) = (
        view_uniforms.uniforms.binding(),
        particle_buffers.and_then(|b| gpu_buffers.get(&b.particles)),
    ) else {
        commands.remove_resource::<DrawBindGroup>();
        return;
    };
    let bind_group = device.create_bind_group(
        "particle_draw_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((view_binding, particles.buffer.as_entire_binding())),
    );
    commands.insert_resource(DrawBindGroup(bind_group));
}

fn draw_particles(
    view: ViewQuery<(&ViewTarget, &ViewUniformOffset, &ViewDrawPipeline)>,
    pipeline_cache: Res<PipelineCache>,
    bind_group: Option<Res<DrawBindGroup>>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    mut ctx: RenderContext,
) {
    let (target, view_offset, view_pipeline) = view.into_inner();
    let (Some(bind_group), Some(particle_buffers), Some(render_pipeline)) = (
        bind_group,
        particle_buffers,
        pipeline_cache.get_render_pipeline(view_pipeline.0),
    ) else {
        return;
    };

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
    pass.set_bind_group(0, &bind_group.0, &[view_offset.offset]);
    pass.draw(0..6, 0..particle_buffers.capacity);
}
