//! Draws each space as one item of Bevy's transparent 2D phase, so that spaces sort by z among
//! sprites and meshes.

use bevy::{
    asset::load_embedded_asset,
    camera::visibility::RenderLayers,
    core_pipeline::core_2d::{Transparent2d, CORE_2D_DEPTH_FORMAT},
    ecs::{
        entity::EntityHashMap,
        query::ROQueryItem,
        system::{
            lifetimeless::{Read, SRes},
            SystemParamItem,
        },
    },
    math::FloatOrd,
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::{
            binding_types::{
                sampler, storage_buffer_read_only_sized, texture_2d, uniform_buffer,
            },
            *,
        },
        renderer::{RenderDevice, RenderQueue},
        storage::GpuShaderBuffer,
        texture::{FallbackImage, GpuImage},
        view::{ExtractedView, Msaa, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderStartup, RenderSystems,
    },
};

use super::compute::SpaceGpus;
use crate::{
    render::{BlendMode, GpuRenderParams},
    space::SpaceUploads,
};

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SpecializedRenderPipelines<DrawPipeline>>()
        .init_resource::<SpaceDraws>()
        .add_render_command::<Transparent2d, DrawParticles>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (
                queue_spaces.in_set(RenderSystems::Queue),
                prepare_space_draws.in_set(RenderSystems::PrepareBindGroups),
            ),
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
    blend: BlendMode,
}

fn blend_state(blend: BlendMode) -> BlendState {
    match blend {
        BlendMode::Alpha => BlendState::ALPHA_BLENDING,
        BlendMode::Additive => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
            // Light doesn't make the target any more opaque.
            alpha: BlendComponent {
                src_factor: BlendFactor::Zero,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
        },
    }
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
                    blend: Some(blend_state(key.blend)),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            // Like sprites: hidden by opaque meshes in front, but never hides anything itself.
            depth_stencil: Some(DepthStencilState {
                format: CORE_2D_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            multisample: MultisampleState {
                count: key.samples,
                ..default()
            },
            ..default()
        }
    }
}

fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    let layout = BindGroupLayoutDescriptor::new(
        "particle_draw_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                uniform_buffer::<ViewUniform>(true),
                storage_buffer_read_only_sized(false, None),
                uniform_buffer::<GpuRenderParams>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // live list
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    );
    commands.insert_resource(DrawPipeline {
        layout,
        shader: load_embedded_asset!(asset_server.as_ref(), "shaders/particle_draw.wesl"),
    });
}

/// Adds every space that a view can see to the view's transparent phase.
fn queue_spaces(
    draw_functions: Res<DrawFunctions<Transparent2d>>,
    pipeline: Res<DrawPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<DrawPipeline>>,
    spaces: Res<SpaceUploads>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    views: Query<(&ExtractedView, &Msaa, Option<&RenderLayers>)>,
) {
    let draw_function = draw_functions.read().id::<DrawParticles>();
    for (view, msaa, layers) in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let layers = layers.unwrap_or_default();
        for space in &spaces.0 {
            if !space.visible || !layers.intersects(&space.layers) {
                continue;
            }
            let key = DrawPipelineKey {
                format: view.target_format,
                samples: msaa.samples(),
                blend: space.render.blend,
            };
            phase.add_transient(Transparent2d {
                sort_key: FloatOrd(space.z),
                // Spaces have no render-world entity; the main one tells them apart.
                entity: (Entity::PLACEHOLDER, space.entity.into()),
                pipeline: pipelines.specialize(&pipeline_cache, &pipeline, key),
                draw_function,
                // Must not be empty, or the item is skipped.
                batch_range: 0..1,
                // Sprites find theirs by this; none is ours.
                extracted_index: usize::MAX,
                extra_index: PhaseItemExtraIndex::None,
                indexed: false,
            });
        }
    }
}

/// What it takes to draw each space, by its main-world entity.
#[derive(Resource, Default)]
struct SpaceDraws(EntityHashMap<SpaceDraw>);

#[derive(Default)]
struct SpaceDraw {
    params: UniformBuffer<GpuRenderParams>,
    /// The bind group and draw arguments. `None` until everything they need is ready.
    ready: Option<(BindGroup, Buffer)>,
}

fn prepare_space_draws(
    pipeline: Res<DrawPipeline>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    view_uniforms: Res<ViewUniforms>,
    spaces: Res<SpaceUploads>,
    space_gpus: Res<SpaceGpus>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    fallback: Res<FallbackImage>,
    mut draws: ResMut<SpaceDraws>,
) {
    draws.0.retain(|entity, _| spaces.0.iter().any(|space| space.entity == *entity));
    let layout = pipeline_cache.get_bind_group_layout(&pipeline.layout);
    for space in &spaces.0 {
        let draw = draws.0.entry(space.entity).or_default();
        draw.params.set(space.render.to_gpu(space.z));
        draw.params.write_buffer(&device, &queue);

        let image = match &space.render.texture {
            Some(texture) => images.get(&texture.image),
            None => Some(&fallback.d2),
        };
        draw.ready = match (
            view_uniforms.uniforms.binding(),
            buffers.get(&space.buffers.particles),
            draw.params.binding(),
            image,
            space_gpus.0.get(&space.buffers.id()),
        ) {
            (Some(view), Some(particles), Some(params), Some(image), Some(gpu)) => {
                let bind_group = device.create_bind_group(
                    "particle_draw_bind_group",
                    &layout,
                    &BindGroupEntries::sequential((
                        view,
                        particles.buffer.as_entire_binding(),
                        params,
                        &image.texture_view,
                        &image.sampler,
                        gpu.live.list.as_entire_binding(),
                    )),
                );
                Some((bind_group, gpu.live.draw_args.clone()))
            }
            _ => None,
        };
    }
}

type DrawParticles = (SetItemPipeline, DrawSpaceParticles);

struct DrawSpaceParticles;

impl<P: PhaseItem> RenderCommand<P> for DrawSpaceParticles {
    type Param = SRes<SpaceDraws>;
    type ViewQuery = Read<ViewUniformOffset>;
    type ItemQuery = ();

    fn render<'w>(
        item: &P,
        view_offset: ROQueryItem<'w, '_, Self::ViewQuery>,
        _: Option<()>,
        draws: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let draw = draws.into_inner().0.get(&item.main_entity().id());
        let Some((bind_group, draw_args)) = draw.and_then(|draw| draw.ready.as_ref()) else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(0, bind_group, &[view_offset.offset]);
        pass.draw_indirect(draw_args, 0);
        RenderCommandResult::Success
    }
}
