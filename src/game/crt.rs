//! A subtle CRT post-process: barrel curvature, scanlines, and a vignette.
//!
//! This is a custom fullscreen pass modeled directly on Bevy's built-in
//! effect-stack node (`bevy_post_process::effect_stack`). It's client-only and
//! runs after chromatic aberration, before tonemapping. Enabled per-camera by the
//! [`Crt`] marker component. The shader takes no uniforms, so the node is as
//! small as a custom post-process can be.

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_2d::graph::{Core2d, Node2d};
use bevy::ecs::query::QueryItem;
use bevy::ecs::system::lifetimeless::Read;
use bevy::image::BevyDefault;
use bevy::prelude::*;
use bevy::render::{
    Render, RenderApp, RenderStartup, RenderSystems,
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_graph::{
        NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
    },
    render_resource::{
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        CachedRenderPipelineId, ColorTargetState, ColorWrites, FilterMode, FragmentState,
        Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
        RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
        SpecializedRenderPipeline, SpecializedRenderPipelines, TextureFormat, TextureSampleType,
        binding_types::{sampler, texture_2d},
    },
    renderer::{RenderContext, RenderDevice},
    view::{ExtractedView, ViewTarget},
};
use bevy::shader::Shader;

/// Marker that enables the CRT post-process on a camera.
#[derive(Component, Clone, Copy, Default)]
pub struct Crt;

impl ExtractComponent for Crt {
    type QueryData = Read<Crt>;
    type QueryFilter = With<Camera>;
    type Out = Crt;

    fn extract_component(_: QueryItem<'_, '_, Self::QueryData>) -> Option<Crt> {
        Some(Crt)
    }
}

#[derive(RenderLabel, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CrtLabel;

pub struct CrtPlugin;

impl Plugin for CrtPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "crt.wgsl");
        app.add_plugins(ExtractComponentPlugin::<Crt>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_resource::<SpecializedRenderPipelines<CrtPipeline>>()
            .add_systems(RenderStartup, init_crt_pipeline)
            .add_systems(Render, prepare_crt_pipelines.in_set(RenderSystems::Prepare))
            .add_render_graph_node::<ViewNodeRunner<CrtNode>>(Core2d, CrtLabel)
            // Run after chromatic aberration, before tonemapping.
            .add_render_graph_edges(
                Core2d,
                (Node2d::PostProcessing, CrtLabel, Node2d::Tonemapping),
            );
    }
}

#[derive(Resource)]
struct CrtPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
}

fn init_crt_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "crt bind group layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                // The screen texture and its sampler.
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );

    let sampler = render_device.create_sampler(&SamplerDescriptor {
        mipmap_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        ..default()
    });

    commands.insert_resource(CrtPipeline {
        layout,
        sampler,
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: load_embedded_asset!(asset_server.as_ref(), "crt.wgsl"),
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CrtPipelineKey {
    texture_format: TextureFormat,
}

impl SpecializedRenderPipeline for CrtPipeline {
    type Key = CrtPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("crt".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(),
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: key.texture_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

#[derive(Component, Deref)]
struct CrtPipelineId(CachedRenderPipelineId);

fn prepare_crt_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<CrtPipeline>>,
    crt_pipeline: Res<CrtPipeline>,
    views: Query<(Entity, &ExtractedView), With<Crt>>,
) {
    for (entity, view) in views.iter() {
        let pipeline_id = pipelines.specialize(
            &pipeline_cache,
            &crt_pipeline,
            CrtPipelineKey {
                texture_format: if view.hdr {
                    ViewTarget::TEXTURE_FORMAT_HDR
                } else {
                    TextureFormat::bevy_default()
                },
            },
        );
        commands.entity(entity).insert(CrtPipelineId(pipeline_id));
    }
}

#[derive(Default)]
struct CrtNode;

impl ViewNode for CrtNode {
    type ViewQuery = (Read<ViewTarget>, Read<CrtPipelineId>);

    fn run<'w>(
        &self,
        _: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (view_target, pipeline_id): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let pipeline_cache = world.resource::<PipelineCache>();
        let crt_pipeline = world.resource::<CrtPipeline>();

        let Some(pipeline) = pipeline_cache.get_render_pipeline(**pipeline_id) else {
            return Ok(());
        };

        // Ping-pong full-screen pass: read `source`, write `destination`.
        let post_process = view_target.post_process_write();

        let bind_group = render_context.render_device().create_bind_group(
            Some("crt bind group"),
            &pipeline_cache.get_bind_group_layout(&crt_pipeline.layout),
            &BindGroupEntries::sequential((post_process.source, &crt_pipeline.sampler)),
        );

        let mut render_pass =
            render_context
                .command_encoder()
                .begin_render_pass(&RenderPassDescriptor {
                    label: Some("crt"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: post_process.destination,
                        depth_slice: None,
                        resolve_target: None,
                        ops: Operations::default(),
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

        render_pass.set_pipeline(pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1);

        Ok(())
    }
}
