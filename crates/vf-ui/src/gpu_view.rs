//! Optional GPU viewport rendered with wgpu (Vulkan on Linux/Windows).
//!
//! The deterministic CPU renderer remains the reference for final images.
//! This module adds a fast interactive 3D view: the heightfield is uploaded as
//! a terrain mesh and drawn into an offscreen texture with a depth buffer,
//! then blitted into egui. Using an offscreen target is necessary because
//! egui's render pass has no depth attachment.
//!
//! The mesh applies the modifier stack and the cliff pass, mirroring the
//! geometry used by the CPU preview. Continuous rock detail, clouds, shadows
//! and AO are intentionally omitted: this is a navigation preview, not the
//! final render.

use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use glam::{DVec3, Mat4, Vec3};
use vf_core::{Camera, HeightField, Project};
use vf_detail::DetailSampler;
use vf_material::{MaterialSampler, RockTexture};
use wgpu::util::DeviceExt as _;

const MAX_MESH_SEGMENTS: usize = 2049;
const SCENE_SHADER: &str = include_str!("shaders/scene.wgsl");
const BLIT_SHADER: &str = include_str!("shaders/blit.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TerrainVertex {
    position: [f32; 3],
    normal: [f32; 3],
    kind: f32,
    height: f32,
}

/// CPU-side mesh produced by [`build_terrain_mesh`]. It can be built on a
/// worker thread and uploaded later, so the GPU mesh refresh never blocks the
/// message pump on a large terrain with modifiers.
pub struct TerrainMeshData {
    vertices: Vec<TerrainVertex>,
    indices: Vec<u32>,
    origin: DVec3,
    min_z: f64,
    max_z: f64,
    summary: String,
    sea_level: f64,
    spacing_x: f64,
    spacing_y: f64,
    width: usize,
    height: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuUniforms {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    sky_zenith: [f32; 4],
    sky_horizon: [f32; 4],
    fog_color: [f32; 4],
    sand: [f32; 4],
    grass: [f32; 4],
    rock: [f32; 4],
    snow: [f32; 4],
    water_deep: [f32; 4],
    water_shallow: [f32; 4],
    params: [f32; 4],
    misc: [f32; 4],
    material: [f32; 4],
    material2: [f32; 4],
    material3: [f32; 4],
}

/// A live GPU viewport. Dropping it releases all wgpu resources.
pub struct GpuViewport {
    shared: Arc<Mutex<GpuRenderer>>,
    label: String,
}

impl GpuViewport {
    /// Build the pipelines and upload the initial terrain.
    pub fn new(
        render_state: &egui_wgpu::RenderState,
        project: &Project,
        heightfield: &HeightField,
        water_enabled: bool,
    ) -> Result<Self, String> {
        let info = render_state.adapter.get_info();
        let backend = match info.backend {
            wgpu::Backend::Vulkan => "Vulkan",
            wgpu::Backend::Metal => "Metal",
            wgpu::Backend::Dx12 => "DX12",
            wgpu::Backend::Gl => "OpenGL",
            wgpu::Backend::BrowserWebGpu => "WebGPU",
            wgpu::Backend::Noop => "Noop",
        };
        let label = format!("{} · {backend}", info.name);
        let renderer = GpuRenderer::new(render_state, project, heightfield, water_enabled)?;
        Ok(Self {
            shared: Arc::new(Mutex::new(renderer)),
            label,
        })
    }

    /// Human-readable adapter/backend label for the UI.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Upload CPU mesh data built off the UI thread.
    pub fn upload_mesh(&self, data: TerrainMeshData) -> Result<(), String> {
        let mut renderer = self
            .shared
            .lock()
            .map_err(|_| "GPU state lock poisoned".to_string())?;
        renderer.upload_mesh_data(data)
    }

    /// Build the per-frame paint callback for an egui rectangle.
    pub fn callback(
        &self,
        camera: &Camera,
        project: &Project,
        width: u32,
        height: u32,
    ) -> Option<impl egui_wgpu::CallbackTrait + 'static> {
        let uniforms = self
            .shared
            .lock()
            .ok()?
            .uniforms(camera, project, width, height);
        Some(GpuCallback {
            shared: Arc::clone(&self.shared),
            uniforms,
            width: width.max(1),
            height: height.max(1),
        })
    }

    /// Short description of the uploaded mesh, for diagnostics/UI.
    pub fn summary(&self) -> String {
        self.shared.lock().map_or_else(
            |_| "GPU indisponible".to_string(),
            |r| r.mesh_summary.clone(),
        )
    }
}

struct GpuCallback {
    shared: Arc<Mutex<GpuRenderer>>,
    uniforms: GpuUniforms,
    width: u32,
    height: u32,
}

impl egui_wgpu::CallbackTrait for GpuCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Ok(mut renderer) = self.shared.lock() {
            renderer.prepare(&self.uniforms, self.width, self.height, encoder);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        if let Ok(renderer) = self.shared.lock() {
            renderer.paint(info, render_pass);
        }
    }
}

struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    scene_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    offscreen: Option<Offscreen>,
    origin: DVec3,
    min_z: f64,
    relief: f64,
    sea_level: f64,
    world_extent: f64,
    mesh_summary: String,
}

struct Offscreen {
    width: u32,
    height: u32,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    _color: wgpu::Texture,
    _depth: wgpu::Texture,
}

impl GpuRenderer {
    fn new(
        render_state: &egui_wgpu::RenderState,
        project: &Project,
        heightfield: &HeightField,
        water_enabled: bool,
    ) -> Result<Self, String> {
        let device = render_state.device.clone();
        let queue = render_state.queue.clone();
        let target_format = render_state.target_format;

        let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vistaforge-gpu-scene"),
            source: wgpu::ShaderSource::Wgsl(SCENE_SHADER.into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vistaforge-gpu-blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vistaforge-gpu-uniform-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vistaforge-gpu-scene-layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TerrainVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3,
                1 => Float32x3,
                2 => Float32,
                3 => Float32
            ],
        };
        let color_target = wgpu::ColorTargetState {
            format: target_format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        };
        let depth_state = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };

        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vistaforge-gpu-terrain"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_terrain"),
                buffers: &[Some(vertex_layout)],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(depth_state.clone()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some("fs_terrain"),
                targets: &[Some(color_target.clone())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vistaforge-gpu-sky"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                ..depth_state.clone()
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some("fs_sky"),
                targets: &[Some(color_target.clone())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vistaforge-gpu-blit-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vistaforge-gpu-blit-pipeline-layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vistaforge-gpu-blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_blit"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_blit"),
                targets: &[Some(color_target)],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vistaforge-gpu-uniforms"),
            size: std::mem::size_of::<GpuUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vistaforge-gpu-uniform-bind-group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vistaforge-gpu-blit-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vistaforge-gpu-empty-vertices"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vistaforge-gpu-empty-indices"),
            size: 16,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut renderer = Self {
            device,
            queue,
            target_format,
            scene_pipeline,
            sky_pipeline,
            blit_pipeline,
            uniform_buffer,
            uniform_bind_group,
            blit_layout,
            sampler,
            vertex_buffer,
            index_buffer,
            index_count: 0,
            offscreen: None,
            origin: DVec3::ZERO,
            min_z: 0.0,
            relief: 1.0,
            sea_level: 0.0,
            world_extent: 1.0,
            mesh_summary: String::new(),
        };
        renderer.upload_terrain(project, heightfield, water_enabled)?;
        Ok(renderer)
    }

    fn upload_terrain(
        &mut self,
        project: &Project,
        heightfield: &HeightField,
        water_enabled: bool,
    ) -> Result<(), String> {
        let data = build_terrain_mesh(project, heightfield, water_enabled)?;
        self.upload_mesh_data(data)
    }

    fn upload_mesh_data(&mut self, data: TerrainMeshData) -> Result<(), String> {
        if data.vertices.is_empty() || data.indices.is_empty() {
            return Err("terrain mesh is empty".into());
        }
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vistaforge-gpu-terrain-vertices"),
                contents: bytemuck::cast_slice(&data.vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vistaforge-gpu-terrain-indices"),
                contents: bytemuck::cast_slice(&data.indices),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            });
        self.vertex_buffer = vertex_buffer;
        self.index_buffer = index_buffer;
        self.index_count = data.indices.len() as u32;
        self.origin = data.origin;
        self.min_z = data.min_z;
        self.relief = (data.max_z - data.min_z).max(1.0);
        self.sea_level = data.sea_level;
        self.world_extent = ((data.width as f64 - 1.0) * data.spacing_x)
            .max((data.height as f64 - 1.0) * data.spacing_y)
            .max(self.relief * 1.2)
            .max(1.0);
        self.mesh_summary = data.summary;
        Ok(())
    }

    fn uniforms(&self, camera: &Camera, project: &Project, width: u32, height: u32) -> GpuUniforms {
        let eye = camera.position - self.origin;
        let target = camera.target - self.origin;
        let distance = (camera.target - camera.position).length().max(0.1);
        let near = (distance * 0.002).max(0.05);
        let far = (distance * 250.0 + self.relief.max(1.0) * 30.0).max(100.0);
        let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
        let fov = camera.fov_y_rad();
        let view = Mat4::look_at_rh(eye.as_vec3(), target.as_vec3(), camera.up.as_vec3());
        let projection = Mat4::perspective_rh(
            camera.fov_y_deg.to_radians(),
            aspect as f32,
            near as f32,
            far as f32,
        );
        let view_proj = projection * view;
        let inv_view_proj = view_proj.inverse();
        let forward = camera.forward().as_vec3();
        let right = camera.right().as_vec3();
        let up = camera.true_up().as_vec3();
        let sun_dir = project.sun.direction().as_vec3();
        let sun_color = project.sun.color().scale(project.sun.intensity);
        let palette = &project.palette;
        let fog_density = 0.18 / self.world_extent.max(1.0);
        let sea_rel = (self.sea_level - self.min_z) as f32;
        let tint = vf_material::RockStyle::preset(&project.materials.rock_preset).tint;
        let rock_tint = [
            palette.rock.r * tint[0],
            palette.rock.g * tint[1],
            palette.rock.b * tint[2],
        ];
        let encode_srgb = if self.target_format.is_srgb() {
            0.0
        } else {
            1.0
        };
        GpuUniforms {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            eye: [eye.x as f32, eye.y as f32, eye.z as f32, 1.0],
            forward: [forward.x, forward.y, forward.z, 0.0],
            right: [right.x, right.y, right.z, 0.0],
            up: [up.x, up.y, up.z, 0.0],
            sun_dir: [sun_dir.x, sun_dir.y, sun_dir.z, 0.0],
            sun_color: [sun_color.r, sun_color.g, sun_color.b, 1.0],
            sky_zenith: rgba(palette.sky_zenith),
            sky_horizon: rgba(palette.sky_horizon),
            fog_color: rgba(palette.fog),
            sand: rgba(palette.sand),
            grass: rgba(palette.grass),
            rock: rgba(palette.rock),
            snow: rgba(palette.snow),
            water_deep: rgba(palette.water_deep),
            water_shallow: rgba(palette.water_shallow),
            params: [
                (fov * 0.5).tan() as f32,
                aspect as f32,
                project.render.exposure.max(0.01),
                encode_srgb,
            ],
            misc: [sea_rel, self.relief as f32, fog_density as f32, 0.0],
            material: [
                if project.materials.enabled { 1.0 } else { 0.0 },
                project.materials.bump_strength,
                project.materials.scree,
                project.materials.snow_coverage,
            ],
            material2: [
                project.materials.vegetation,
                project.materials.wetness,
                ((project.style.snow_line_m - self.min_z as f32) / self.relief as f32)
                    .clamp(0.0, 1.0),
                ((project.style.beach_height_m - self.min_z as f32) / self.relief as f32)
                    .clamp(0.0, 1.0),
            ],
            material3: [
                rock_tint[0],
                rock_tint[1],
                rock_tint[2],
                project.materials.bump_scale_m.max(0.05),
            ],
        }
    }

    fn ensure_offscreen(&mut self, width: u32, height: u32) {
        if self
            .offscreen
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return;
        }
        let width = width.clamp(1, 8192);
        let height = height.clamp(1, 8192);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vistaforge-gpu-offscreen-color"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vistaforge-gpu-offscreen-depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vistaforge-gpu-blit-bind-group"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.offscreen = Some(Offscreen {
            width,
            height,
            color_view,
            depth_view,
            bind_group,
            _color: color,
            _depth: depth,
        });
    }

    fn prepare(
        &mut self,
        uniforms: &GpuUniforms,
        width: u32,
        height: u32,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        self.ensure_offscreen(width, height);
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(uniforms));
        let Some(offscreen) = &self.offscreen else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vistaforge-gpu-scene-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &offscreen.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.02,
                        g: 0.03,
                        b: 0.05,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &offscreen.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        pass.set_pipeline(&self.sky_pipeline);
        pass.draw(0..3, 0..1);
        if self.index_count > 0 {
            pass.set_pipeline(&self.scene_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>) {
        let Some(offscreen) = &self.offscreen else {
            return;
        };
        let viewport = info.viewport_in_pixels();
        if viewport.width_px == 0 || viewport.height_px == 0 {
            return;
        }
        pass.set_viewport(
            viewport.left_px as f32,
            viewport.top_px as f32,
            viewport.width_px as f32,
            viewport.height_px as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &offscreen.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn rgba(color: vf_core::Rgb) -> [f32; 4] {
    [color.r, color.g, color.b, 1.0]
}

fn push_water_plane(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    project: &Project,
    extent: f64,
    relief: f64,
    min_z: f64,
) {
    if !project.water.enabled {
        return;
    }
    let sea = f64::from(project.water.sea_level);
    if sea <= min_z - relief * 0.05 {
        return;
    }
    let half = extent * 12.0;
    let sea_rel = (sea - min_z - relief * 0.0005) as f32;
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&[
        TerrainVertex {
            position: [-half as f32, -half as f32, sea_rel],
            normal: [0.0, 0.0, 1.0],
            kind: 1.0,
            height: sea_rel,
        },
        TerrainVertex {
            position: [half as f32, -half as f32, sea_rel],
            normal: [0.0, 0.0, 1.0],
            kind: 1.0,
            height: sea_rel,
        },
        TerrainVertex {
            position: [half as f32, half as f32, sea_rel],
            normal: [0.0, 0.0, 1.0],
            kind: 1.0,
            height: sea_rel,
        },
        TerrainVertex {
            position: [-half as f32, half as f32, sea_rel],
            normal: [0.0, 0.0, 1.0],
            kind: 1.0,
            height: sea_rel,
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

pub fn build_terrain_mesh(
    project: &Project,
    heightfield: &HeightField,
    water_enabled: bool,
) -> Result<TerrainMeshData, String> {
    let modulated = if project.modifiers.active().next().is_some() {
        Some(
            vf_modifiers::apply(heightfield, &project.modifiers, project.seed)
                .map_err(|error| format!("modifiers: {error}"))?,
        )
    } else {
        None
    };
    let base = modulated.as_ref().unwrap_or(heightfield);
    let shaped = if project.render.cliffs {
        Some(
            vf_procgen::cliff::sharpen(
                base,
                project.seed,
                project.render.cliff_warp,
                project.render.cliff_warp_scale_m,
            )
            .map_err(|error| format!("cliff shaping: {error}"))?,
        )
    } else {
        None
    };
    let terrain = shaped.as_ref().unwrap_or(base);
    // The GPU preview now evaluates the same continuous detail sampler as the
    // CPU renderer. Without this, every Detail-menu change was invisible in
    // the live 3D view even though the final render used it.
    let detail_sampler = project
        .detail
        .enabled
        .then(|| DetailSampler::new(terrain, &project.detail, project.seed));

    let width = terrain.width();
    let height = terrain.height();
    if width < 2 || height < 2 {
        return Err("heightfield is too small".into());
    }
    let (spacing_x, spacing_y) = terrain.spacing();
    let (origin_x, origin_y) = terrain.origin();
    let bounds = terrain.bounds();
    let min_z = bounds.min[2];
    let max_z = bounds.max[2];
    let center_x = (bounds.min[0] + bounds.max[0]) * 0.5;
    let center_y = (bounds.min[1] + bounds.max[1]) * 0.5;
    let origin = DVec3::new(center_x, center_y, 0.0);
    let relief = (max_z - min_z).max(1.0);
    let extent = bounds.size().iter().copied().fold(1.0, f64::max);

    // Shape-adaptive path: mirror the CPU renderer so the Detail menu's
    // adaptive/quads settings are visible in the live viewport too.
    if project.detail.adaptive {
        let sampler = DetailSampler::new(terrain, &project.detail, project.seed);
        let material_texture = RockTexture::load(&project.materials.texture_path);
        let displace = project.materials.enabled
            && (project.materials.displacement_m > 0.0
                || project.materials.texture_displacement_m > 0.0);
        let material_sampler = displace.then(|| {
            MaterialSampler::new(
                &sampler,
                &project.materials,
                project.seed,
                material_texture.as_ref(),
            )
        });
        let tolerance = if displace {
            let amplitude = project
                .materials
                .displacement_m
                .max(project.materials.texture_displacement_m);
            project
                .detail
                .adaptive_tolerance_m
                .min((amplitude * 0.75).max(0.05))
        } else {
            project.detail.adaptive_tolerance_m
        };
        let subdivision = project.render.subdivision.max(if displace { 2 } else { 1 }) as u32;
        let surface: &dyn vf_mesh::SurfaceSampler = match &material_sampler {
            Some(sampler) => sampler,
            None => &sampler,
        };
        let mesh = vf_mesh::build_shape_adaptive(surface, subdivision, tolerance, 1_800_000);
        let mut vertices = Vec::with_capacity(mesh.vertex_count() + 4);
        let mut indices = Vec::with_capacity(mesh.triangle_count() * 3 + 6);
        for (index, position) in mesh.positions.iter().enumerate() {
            let normal = mesh.normals[index];
            vertices.push(TerrainVertex {
                position: [
                    position[0] - center_x as f32,
                    position[1] - center_y as f32,
                    (f64::from(position[2]) - min_z) as f32,
                ],
                normal,
                kind: 0.0,
                height: (f64::from(position[2]) - min_z) as f32,
            });
        }
        for [a, b, c] in &mesh.indices {
            indices.extend_from_slice(&[*a, *b, *c]);
        }
        if water_enabled {
            push_water_plane(&mut vertices, &mut indices, project, extent, relief, min_z);
        }
        let triangles = indices.len() / 3;
        return Ok(TerrainMeshData {
            vertices,
            indices,
            origin,
            min_z,
            max_z,
            summary: format!(
                "adaptatif {}×{} -> {} sommets · {} triangles",
                width,
                height,
                mesh.vertex_count(),
                triangles
            ),
            sea_level: f64::from(project.water.sea_level),
            spacing_x,
            spacing_y,
            width,
            height,
        });
    }

    let longest = width.max(height) - 1;
    // One cap for both paths: enabling the detail pass used to halve the
    // terrain resolution exactly when the user asked for more detail, which
    // made the preview look *smoother* with detail on than with it off.
    let step = longest.div_ceil(MAX_MESH_SEGMENTS).max(1);
    let cols = (width - 1).div_ceil(step) + 1;
    let rows = (height - 1).div_ceil(step) + 1;
    let sample = |x: usize, y: usize| f64::from(terrain.heights()[y * width + x]);
    // The final render evaluates the detail at half a cell (`vf-mesh`
    // `build_shape_adaptive`), not at twice the display step: use the same
    // footprint so the preview attenuates the same wavelengths as the PNG.
    let footprint = (0.5 * spacing_x.max(spacing_y)).max(0.05) as f32;

    let mut vertices = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        let y = (row * step).min(height - 1);
        for col in 0..cols {
            let x = (col * step).min(width - 1);
            let world_x = origin_x + x as f64 * spacing_x;
            let world_y = origin_y + y as f64 * spacing_y;
            let (z, normal) = if let Some(sampler) = &detail_sampler {
                (
                    f64::from(sampler.height(world_x, world_y, footprint)),
                    sampler.normal(world_x, world_y, footprint),
                )
            } else {
                let z = sample(x, y);
                // Difference on the native grid: a ±step stencil smoothed away
                // every ridge narrower than two display quads.
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(width - 1);
                let ym = y.saturating_sub(1);
                let yp = (y + 1).min(height - 1);
                let dx = ((xp - xm) as f64 * spacing_x).max(f64::EPSILON);
                let dy = ((yp - ym) as f64 * spacing_y).max(f64::EPSILON);
                let dzdx = (sample(xp, y) - sample(xm, y)) / dx;
                let dzdy = (sample(x, yp) - sample(x, ym)) / dy;
                let normal = Vec3::new(-dzdx as f32, -dzdy as f32, 1.0).normalize();
                (z, normal.to_array())
            };
            vertices.push(TerrainVertex {
                position: [
                    (world_x - center_x) as f32,
                    (world_y - center_y) as f32,
                    (z - min_z) as f32,
                ],
                normal,
                kind: 0.0,
                height: (z - min_z) as f32,
            });
        }
    }

    let mut indices = Vec::with_capacity((cols - 1) * (rows - 1) * 6);
    for row in 0..rows - 1 {
        for col in 0..cols - 1 {
            let a = (row * cols + col) as u32;
            let b = a + 1;
            let c = a + cols as u32;
            let d = c + 1;
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }

    // A large opaque water plane anchors the terrain in the landscape. The
    // CPU renderer has Fresnel/foam; the GPU preview uses a simpler shader.
    if water_enabled {
        push_water_plane(&mut vertices, &mut indices, project, extent, relief, min_z);
    }

    let triangles = indices.len() / 3;
    let summary = format!(
        "{}×{} -> {}×{} sommets · {} triangles",
        width, height, cols, rows, triangles
    );
    Ok(TerrainMeshData {
        vertices,
        indices,
        origin,
        min_z,
        max_z,
        summary,
        sea_level: f64::from(project.water.sea_level),
        spacing_x,
        spacing_y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::Seed;

    fn terrain(n: usize, spacing: f64) -> HeightField {
        let heights = (0..n * n)
            .map(|index| {
                let x = (index % n) as f32;
                let y = (index / n) as f32;
                x * 4.0 + y * 2.0 + (x * 0.3).sin() * 8.0
            })
            .collect();
        HeightField::new(n, n, spacing, spacing, heights).unwrap()
    }

    fn project() -> Project {
        let mut project = Project::new("gpu-test", Seed::new(7), 65);
        project.water.enabled = true;
        project.water.sea_level = 20.0;
        project
    }

    #[test]
    fn mesh_contains_terrain_and_water() {
        let heightfield = terrain(65, 10.0);
        let mesh = build_terrain_mesh(&project(), &heightfield, true).unwrap();
        let (vertices, indices) = (&mesh.vertices, &mesh.indices);
        assert!(vertices.iter().any(|vertex| vertex.kind == 0.0));
        assert!(vertices.iter().any(|vertex| vertex.kind == 1.0));
        assert_eq!(indices.len() % 3, 0);
        let max_index = *indices.iter().max().unwrap() as usize;
        assert!(max_index < vertices.len());
    }

    #[test]
    fn water_can_be_disabled() {
        let heightfield = terrain(65, 10.0);
        let mut project = project();
        project.water.enabled = false;
        let mesh = build_terrain_mesh(&project, &heightfield, true).unwrap();
        let vertices = &mesh.vertices;
        assert!(vertices.iter().all(|vertex| vertex.kind == 0.0));
    }

    #[test]
    fn water_ui_flag_removes_water() {
        let heightfield = terrain(65, 10.0);
        let mesh = build_terrain_mesh(&project(), &heightfield, false).unwrap();
        let vertices = &mesh.vertices;
        assert!(vertices.iter().all(|vertex| vertex.kind == 0.0));
    }

    #[test]
    fn mesh_resolution_is_capped() {
        let heightfield = terrain(1300, 5.0);
        let mesh = build_terrain_mesh(&project(), &heightfield, true).unwrap();
        let vertices = &mesh.vertices;
        let max = (MAX_MESH_SEGMENTS + 1) * (MAX_MESH_SEGMENTS + 1) + 4;
        assert!(vertices.len() <= max, "{} vertices", vertices.len());
    }

    #[test]
    fn mesh_normals_are_finite_and_upward() {
        let heightfield = terrain(65, 10.0);
        let mesh = build_terrain_mesh(&project(), &heightfield, true).unwrap();
        let vertices = &mesh.vertices;
        for vertex in vertices.iter().filter(|v| v.kind == 0.0) {
            assert!(vertex.normal.iter().all(|value| value.is_finite()));
            assert!(vertex.normal[2] > 0.0);
        }
    }
}
