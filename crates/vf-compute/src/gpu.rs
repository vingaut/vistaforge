//! wgpu compute backend for the geometry kernels.
//!
//! The module owns one wgpu instance, opens a device per selected adapter and
//! caches the compute pipelines. Every kernel is dispatched per vertex or per
//! face, so splitting a vertex range between several adapters is safe: each
//! element is written by exactly one device and the CPU assembly is a plain
//! copy, which keeps the multi-GPU path deterministic.
//!
//! A device that cannot be opened, a shader that fails to compile or a lost
//! device never aborts an operation: the caller falls back to the CPU kernels
//! and [`last_error`] explains what happened.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt as _;

use crate::backend::BackendState;
use crate::mesh_graph::{FaceGraph, Neighbours, VertexCorners};

/// Vertices per GPU workgroup. Also the granularity of a dispatch.
const WORKGROUP: u32 = 64;

const NORMALS_PASS: &str = include_str!("shaders/normals_pass.wgsl");
const VERTEX_NORMALS: &str = include_str!("shaders/vertex_normals.wgsl");
const SMOOTH: &str = include_str!("shaders/smooth.wgsl");
const DISPLACE: &str = include_str!("shaders/displace.wgsl");
const TRANSFORM: &str = include_str!("shaders/transform.wgsl");

// ---------------------------------------------------------------------------
// Adapters
// ---------------------------------------------------------------------------

/// One compute-capable adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterChoice {
    /// Stable key, `backend|name`, shared with the viewport GPU preference.
    pub key: String,
    pub name: String,
    pub backend: String,
    pub device_type: String,
}

impl AdapterChoice {
    pub fn label(&self) -> String {
        format!("{} · {}", self.name, self.backend)
    }
}

fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Gl => "OpenGL",
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Noop => "Noop",
    }
}

fn device_type_name(kind: wgpu::DeviceType) -> &'static str {
    match kind {
        wgpu::DeviceType::Cpu => "CPU",
        wgpu::DeviceType::IntegratedGpu => "GPU intégré",
        wgpu::DeviceType::DiscreteGpu => "GPU dédié",
        wgpu::DeviceType::VirtualGpu => "GPU virtuel",
        wgpu::DeviceType::Other => "Autre",
    }
}

fn instance() -> &'static wgpu::Instance {
    static INSTANCE: OnceLock<wgpu::Instance> = OnceLock::new();
    INSTANCE
        .get_or_init(|| wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()))
}

/// Device opening and enumeration are serialised: the Vulkan/DX12 loaders and
/// wgpu's instance are not reliable when several threads enumerate or create a
/// device at the same time. Kernels themselves stay fully concurrent.
fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn adapters() -> Vec<(wgpu::Adapter, AdapterChoice)> {
    let _guard = gpu_lock();
    let list = pollster::block_on(instance().enumerate_adapters(wgpu::Backends::all()));
    let mut out: Vec<(wgpu::Adapter, AdapterChoice)> = Vec::new();
    for adapter in list {
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            // A CPU adapter would only be slower than the rayon pool.
            continue;
        }
        let choice = AdapterChoice {
            key: format!("{}|{}", backend_name(info.backend), info.name),
            name: info.name,
            backend: backend_name(info.backend).to_string(),
            device_type: device_type_name(info.device_type).to_string(),
        };
        if out.iter().any(|(_, existing)| existing.key == choice.key) {
            continue;
        }
        out.push((adapter, choice));
    }
    out
}

/// `(key, label)` pairs for the adapter picker.
type AdapterLabels = Vec<(String, String)>;

/// `(key, label)` for every compute adapter, cached for the session.
///
/// Enumerating the Vulkan and DX12 adapters costs ~100 ms per call, and the
/// status line, the preferences panel and every kernel that asks for "the first
/// adapter" would otherwise pay it again on each call. [`invalidate`] clears it.
fn adapter_labels_cache() -> &'static Mutex<Option<AdapterLabels>> {
    static CACHE: OnceLock<Mutex<Option<AdapterLabels>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn adapter_labels() -> AdapterLabels {
    let cell = adapter_labels_cache();
    if let Ok(guard) = cell.lock()
        && let Some(hit) = guard.as_ref()
    {
        return hit.clone();
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for (_, choice) in adapters() {
        if out
            .iter()
            .any(|(_, label)| label.starts_with(&format!("{} ·", choice.name)))
        {
            continue;
        }
        out.push((choice.key.clone(), choice.label()));
    }
    if let Ok(mut guard) = cell.lock() {
        *guard = Some(out.clone());
    }
    out
}

/// Every compute adapter, `(key, label)`, deduplicated by physical device so a
/// GPU exposed by both Vulkan and DX12 appears once.
pub fn detected() -> Vec<(String, String)> {
    adapter_labels()
}

/// Adapter labels, for diagnostics.
pub fn probe() -> Vec<String> {
    detected().into_iter().map(|(_, label)| label).collect()
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Uniform,
    Read,
    Write,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kernel {
    FaceNormals,
    FaceCross,
    CornerWeights,
    VertexNormals,
    Smooth,
    Displace,
    Transform,
}

impl Kernel {
    fn id(self) -> &'static str {
        match self {
            Self::FaceNormals => "face-normals",
            Self::FaceCross => "face-cross",
            Self::CornerWeights => "corner-weights",
            Self::VertexNormals => "vertex-normals",
            Self::Smooth => "smooth",
            Self::Displace => "displace",
            Self::Transform => "transform",
        }
    }

    fn shader(self) -> &'static str {
        match self {
            Self::FaceNormals | Self::FaceCross | Self::CornerWeights => NORMALS_PASS,
            Self::VertexNormals => VERTEX_NORMALS,
            Self::Smooth => SMOOTH,
            Self::Displace => DISPLACE,
            Self::Transform => TRANSFORM,
        }
    }

    /// Entry point in the shader module. The first three share one module and
    /// one binding layout.
    fn entry(self) -> &'static str {
        match self {
            Self::FaceNormals => "face_normals",
            Self::FaceCross => "face_cross",
            Self::CornerWeights => "corner_weights",
            Self::VertexNormals => "vertex_normals",
            Self::Smooth => "smooth_pass",
            Self::Displace => "displace",
            Self::Transform => "transform",
        }
    }

    fn slots(self) -> &'static [Slot] {
        match self {
            Self::FaceNormals | Self::FaceCross | Self::CornerWeights => &[
                Slot::Uniform,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Write,
            ],
            Self::VertexNormals => &[
                Slot::Uniform,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Write,
            ],
            Self::Smooth => &[
                Slot::Uniform,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Write,
            ],
            Self::Displace => &[
                Slot::Uniform,
                Slot::Read,
                Slot::Read,
                Slot::Read,
                Slot::Write,
            ],
            Self::Transform => &[Slot::Uniform, Slot::Read, Slot::Write],
        }
    }
}

struct CachedPipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// A live device and queue.
pub struct GpuContext {
    pub adapter: AdapterChoice,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Mutex<HashMap<Kernel, Arc<CachedPipeline>>>,
}

impl GpuContext {
    fn open(adapter: wgpu::Adapter, choice: AdapterChoice) -> Result<Self, String> {
        let (device, queue) = {
            let _guard = gpu_lock();
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("vistaforge-compute"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))
            .map_err(|error| format!("{} : {error}", choice.label()))?
        };
        Ok(Self {
            adapter: choice,
            device,
            queue,
            pipelines: Mutex::new(HashMap::new()),
        })
    }

    /// Human-readable adapter label.
    pub fn label(&self) -> String {
        self.adapter.label()
    }

    fn pipeline(&self, kernel: Kernel) -> Result<Arc<CachedPipeline>, String> {
        if let Ok(map) = self.pipelines.lock()
            && let Some(cached) = map.get(&kernel)
        {
            return Ok(Arc::clone(cached));
        }
        let layouts: Vec<wgpu::BindGroupLayoutEntry> = kernel
            .slots()
            .iter()
            .enumerate()
            .map(|(binding, slot)| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: match slot {
                    Slot::Uniform => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    Slot::Read => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    Slot::Write => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
                count: None,
            })
            .collect();
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(kernel.id()),
                entries: &layouts,
            });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(kernel.id()),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(kernel.id()),
                source: wgpu::ShaderSource::Wgsl(kernel.shader().into()),
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(kernel.id()),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(kernel.entry()),
                compilation_options: Default::default(),
                cache: None,
            });
        let cached = Arc::new(CachedPipeline { pipeline, layout });
        if let Ok(mut map) = self.pipelines.lock() {
            map.insert(kernel, Arc::clone(&cached));
        }
        Ok(cached)
    }

    /// One compute pass, encoded together with the others of the same kernel.
    ///
    /// Queue submission costs a few hundred microseconds on Windows (WDDM), so
    /// a three-pass kernel must not submit three times: all passes of a kernel
    /// are encoded into one command buffer, the read-back copy is appended, and
    /// only then is the queue submitted and polled once.
    fn run_passes(
        &self,
        passes: &[Pass<'_>],
        readback: Option<(&wgpu::Buffer, u64, u64)>,
    ) -> Result<Vec<f32>, String> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("compute"),
            });
        for pass in passes {
            let cached = self.pipeline(pass.kernel)?;
            let entries: Vec<wgpu::BindGroupEntry<'_>> = pass
                .buffers
                .iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(pass.label),
                layout: &cached.layout,
                entries: &entries,
            });
            let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            compute.set_pipeline(&cached.pipeline);
            compute.set_bind_group(0, &bind_group, &[]);
            compute.dispatch_workgroups(pass.elements.div_ceil(WORKGROUP), 1, 1);
        }
        let staging = match readback {
            Some((buffer, offset_elements, count)) => {
                let size_bytes = count * 4;
                let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: size_bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.copy_buffer_to_buffer(buffer, offset_elements * 4, &staging, 0, size_bytes);
                Some((staging, size_bytes))
            }
            None => None,
        };
        self.queue.submit(Some(encoder.finish()));
        match staging {
            Some((staging, size_bytes)) => self.read_staging(&staging, size_bytes),
            None => Ok(Vec::new()),
        }
    }

    /// Copy `count` f32 out of a staging buffer that has already been submitted.
    fn read_staging(&self, staging: &wgpu::Buffer, size_bytes: u64) -> Result<Vec<f32>, String> {
        if size_bytes == 0 {
            return Ok(Vec::new());
        }
        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("attente GPU : {error}"))?;
        receiver
            .recv()
            .map_err(|_| "lecture GPU interrompue".to_string())?
            .map_err(|error| format!("mappage GPU : {error}"))?;
        let data = slice
            .get_mapped_range()
            .map_err(|error| format!("mappage GPU : {error}"))?;
        let values: Vec<f32> = bytemuck::try_cast_slice::<u8, f32>(&data)
            .map(<[f32]>::to_vec)
            .unwrap_or_else(|_| {
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| f32::from_ne_bytes(*chunk))
                    .collect()
            });
        drop(data);
        staging.unmap();
        Ok(values)
    }

    fn upload<T: bytemuck::Pod>(&self, label: &str, values: &[T]) -> wgpu::Buffer {
        let bytes = bytemuck::cast_slice(values);
        self.upload_bytes(label, bytes, wgpu::BufferUsages::STORAGE)
    }

    /// Upload raw bytes; empty inputs still get a 4-byte buffer because wgpu
    /// rejects zero-sized bindings.
    fn upload_bytes(&self, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        if bytes.is_empty() {
            return self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 4,
                usage,
                mapped_at_creation: false,
            });
        }
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage,
            })
    }

    fn uniform(&self, label: &str, bytes: &[u8]) -> wgpu::Buffer {
        // Uniform binding sizes must be a multiple of 16.
        let mut padded = bytes.to_vec();
        while !padded.len().is_multiple_of(16) || padded.is_empty() {
            padded.push(0);
        }
        self.upload_bytes_(label, &padded, wgpu::BufferUsages::UNIFORM)
    }

    fn upload_bytes_(&self, label: &str, bytes: &[u8], extra: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: extra | wgpu::BufferUsages::COPY_DST,
            })
    }

    fn storage(&self, label: &str, size_bytes: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size_bytes.max(4),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Angle-weighted vertex normals for `range`.
    fn vertex_normals(
        &self,
        positions: &[Vec3],
        graph: &FaceGraph,
        corners: &VertexCorners,
        range: Range<usize>,
    ) -> Result<Vec<Vec3>, String> {
        let vertex_count = positions.len();
        let face_count = graph.face_count();
        let corner_count = graph.corner_count();
        let position_values: Vec<f32> = flatten(positions);
        let positions_buffer = self.upload("positions", &position_values);
        let offsets_buffer = self.upload("face-offsets", graph.face_offsets());
        let corners_buffer = self.upload("corners", graph.corners());
        let corner_faces_buffer = self.upload("corner-faces", graph.corner_faces());
        let vertex_offsets_buffer = self.upload("vertex-offsets", corners.offsets());
        let vertex_corners_buffer = self.upload("vertex-corners", corners.corners());
        let face_normals = self.storage("face-normals", face_count as u64 * 12);
        let corner_weights = self.storage("corner-weights", corner_count as u64 * 4);
        let out_normals = self.storage("vertex-normals", vertex_count as u64 * 12);

        let faces_params = self.uniform("faces-range", &normal_params(0, face_count, false));
        let corners_params = self.uniform("corners-range", &normal_params(0, corner_count, false));
        let vertex_params = self.uniform(
            "vertices-range",
            &normal_params(range.start, range.end, true),
        );
        let passes = [
            Pass {
                kernel: Kernel::FaceNormals,
                label: "face-normals",
                buffers: vec![
                    &faces_params,
                    &positions_buffer,
                    &offsets_buffer,
                    &corners_buffer,
                    &corner_faces_buffer,
                    &face_normals,
                ],
                elements: face_count as u32,
            },
            Pass {
                kernel: Kernel::CornerWeights,
                label: "corner-weights",
                buffers: vec![
                    &corners_params,
                    &positions_buffer,
                    &offsets_buffer,
                    &corners_buffer,
                    &corner_faces_buffer,
                    &corner_weights,
                ],
                elements: corner_count as u32,
            },
            Pass {
                kernel: Kernel::VertexNormals,
                label: "vertex-normals",
                buffers: vec![
                    &vertex_params,
                    &face_normals,
                    &corner_faces_buffer,
                    &corner_weights,
                    &vertex_offsets_buffer,
                    &vertex_corners_buffer,
                    &out_normals,
                ],
                elements: range.len() as u32,
            },
        ];
        let values = self.run_passes(
            &passes,
            Some((&out_normals, range.start as u64 * 3, range.len() as u64 * 3)),
        )?;
        Ok(unflatten(&values))
    }

    /// Unnormalised cross-product vertex normals for `range`.
    fn area_normals(
        &self,
        positions: &[Vec3],
        graph: &FaceGraph,
        corners: &VertexCorners,
        range: Range<usize>,
    ) -> Result<Vec<Vec3>, String> {
        let vertex_count = positions.len();
        let face_count = graph.face_count();
        let position_values = flatten(positions);
        let positions_buffer = self.upload("positions", &position_values);
        let offsets_buffer = self.upload("face-offsets", graph.face_offsets());
        let corners_buffer = self.upload("corners", graph.corners());
        let corner_faces_buffer = self.upload("corner-faces", graph.corner_faces());
        let vertex_offsets_buffer = self.upload("vertex-offsets", corners.offsets());
        let vertex_corners_buffer = self.upload("vertex-corners", corners.corners());
        let face_cross = self.storage("face-cross", face_count as u64 * 12);
        // The gather still declares the corner-weight binding; a zeroed
        // placeholder is harmless because the weighted flag is off.
        let unused_weights = self.storage("unused-weights", 4);
        let out_normals = self.storage("vertex-normals", vertex_count as u64 * 12);
        let faces_params = self.uniform("faces-range", &normal_params(0, face_count, false));
        let vertex_params = self.uniform(
            "vertices-range",
            &normal_params(range.start, range.end, false),
        );
        let passes = [
            Pass {
                kernel: Kernel::FaceCross,
                label: "face-cross",
                buffers: vec![
                    &faces_params,
                    &positions_buffer,
                    &offsets_buffer,
                    &corners_buffer,
                    &corner_faces_buffer,
                    &face_cross,
                ],
                elements: face_count as u32,
            },
            Pass {
                kernel: Kernel::VertexNormals,
                label: "vertex-normals",
                buffers: vec![
                    &vertex_params,
                    &face_cross,
                    &corner_faces_buffer,
                    &unused_weights,
                    &vertex_offsets_buffer,
                    &vertex_corners_buffer,
                    &out_normals,
                ],
                elements: range.len() as u32,
            },
        ];
        let values = self.run_passes(
            &passes,
            Some((&out_normals, range.start as u64 * 3, range.len() as u64 * 3)),
        )?;
        Ok(unflatten(&values))
    }

    /// Laplacian/Taubin passes over `range`, uploaded once.
    fn smooth(
        &self,
        positions: &[Vec3],
        neighbours: &Neighbours,
        passes: &[f32],
        range: Range<usize>,
    ) -> Result<Vec<Vec3>, String> {
        let vertex_count = positions.len();
        let position_values = flatten(positions);
        let first = self.upload_bytes(
            "positions",
            bytemuck::cast_slice(&position_values),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let second = self.storage("positions-out", vertex_count as u64 * 12);
        let offsets = self.upload("neighbour-offsets", neighbours.offsets());
        let neighbour_values = self.upload("neighbours", neighbours.values());
        // The uniforms must outlive the encoded passes.
        let params: Vec<wgpu::Buffer> = passes
            .iter()
            .map(|factor| {
                self.uniform(
                    "smooth-range",
                    &smooth_params(range.start, range.end, *factor),
                )
            })
            .collect();
        let mut direction = true;
        let mut encoded: Vec<Pass<'_>> = Vec::with_capacity(params.len());
        for uniform in &params {
            let (source, target) = if direction {
                (&first, &second)
            } else {
                (&second, &first)
            };
            encoded.push(Pass {
                kernel: Kernel::Smooth,
                label: "smooth",
                buffers: vec![uniform, source, &offsets, &neighbour_values, target],
                elements: range.len() as u32,
            });
            direction = !direction;
        }
        // After an odd number of passes the result sits in `second`.
        let result = if direction { &first } else { &second };
        let values = self.run_passes(
            &encoded,
            Some((result, range.start as u64 * 3, range.len() as u64 * 3)),
        )?;
        Ok(unflatten(&values))
    }

    /// `position + normal * amount` over `range`.
    fn displace(
        &self,
        positions: &[Vec3],
        normals: &[Vec3],
        amounts: &[f32],
        range: Range<usize>,
    ) -> Result<Vec<Vec3>, String> {
        let position_values = flatten(positions);
        let normal_values = flatten(normals);
        let position_buffer = self.upload("positions", &position_values);
        let normal_buffer = self.upload("normals", &normal_values);
        let amount_buffer = self.upload("amounts", amounts);
        let out_buffer = self.storage("positions-out", positions.len() as u64 * 12);
        let params = self.uniform("displace-range", &range_params(range.start, range.end));
        let passes = [Pass {
            kernel: Kernel::Displace,
            label: "displace",
            buffers: vec![
                &params,
                &position_buffer,
                &normal_buffer,
                &amount_buffer,
                &out_buffer,
            ],
            elements: range.len() as u32,
        }];
        let values = self.run_passes(
            &passes,
            Some((&out_buffer, range.start as u64 * 3, range.len() as u64 * 3)),
        )?;
        Ok(unflatten(&values))
    }

    /// Affine transform over `range`.
    fn transform(
        &self,
        positions: &[Vec3],
        matrix: Mat4,
        range: Range<usize>,
    ) -> Result<Vec<Vec3>, String> {
        let position_values = flatten(positions);
        let position_buffer = self.upload("positions", &position_values);
        let out_buffer = self.storage("positions-out", positions.len() as u64 * 12);
        let mut uniform = Vec::with_capacity(80);
        for value in matrix.to_cols_array() {
            uniform.extend_from_slice(&value.to_ne_bytes());
        }
        uniform.extend_from_slice(&range.start.to_ne_bytes());
        uniform.extend_from_slice(&range.end.to_ne_bytes());
        uniform.extend_from_slice(&0u32.to_ne_bytes());
        uniform.extend_from_slice(&0u32.to_ne_bytes());
        let params = self.uniform("transform", &uniform);
        let passes = [Pass {
            kernel: Kernel::Transform,
            label: "transform",
            buffers: vec![&params, &position_buffer, &out_buffer],
            elements: range.len() as u32,
        }];
        let values = self.run_passes(
            &passes,
            Some((&out_buffer, range.start as u64 * 3, range.len() as u64 * 3)),
        )?;
        Ok(unflatten(&values))
    }
}

/// One encoded compute pass: what [`GpuContext::run_passes`] submits.
struct Pass<'a> {
    kernel: Kernel,
    label: &'a str,
    /// Bindings in slot order, matching `Kernel::slots`.
    buffers: Vec<&'a wgpu::Buffer>,
    /// Work items (vertices, faces or corners); the workgroup size is fixed.
    elements: u32,
}

fn flatten(values: &[Vec3]) -> Vec<f32> {
    let mut out = Vec::with_capacity(values.len() * 3);
    for value in values {
        out.push(value.x);
        out.push(value.y);
        out.push(value.z);
    }
    out
}

fn unflatten(values: &[f32]) -> Vec<Vec3> {
    values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|chunk| Vec3::new(chunk[0], chunk[1], chunk[2]))
        .collect()
}

fn range_params(start: usize, end: usize) -> Vec<u8> {
    normal_params(start, end, false)
}

/// Params shared by the normal passes: the range, plus a flag telling the
/// gather whether to use the angle weights.
fn normal_params(start: usize, end: usize, weighted: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&(start as u32).to_ne_bytes());
    bytes.extend_from_slice(&(end as u32).to_ne_bytes());
    bytes.extend_from_slice(&u32::from(weighted).to_ne_bytes());
    bytes.extend_from_slice(&0u32.to_ne_bytes());
    bytes
}

fn smooth_params(start: usize, end: usize, factor: f32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&(start as u32).to_ne_bytes());
    bytes.extend_from_slice(&(end as u32).to_ne_bytes());
    bytes.extend_from_slice(&0u32.to_ne_bytes());
    bytes.extend_from_slice(&0u32.to_ne_bytes());
    bytes.extend_from_slice(&factor.to_ne_bytes());
    bytes.extend_from_slice(&0.0f32.to_ne_bytes());
    bytes.extend_from_slice(&0.0f32.to_ne_bytes());
    bytes.extend_from_slice(&0.0f32.to_ne_bytes());
    bytes
}

// ---------------------------------------------------------------------------
// Device cache
// ---------------------------------------------------------------------------

type Opened = Result<Vec<Arc<GpuContext>>, String>;

fn cache() -> &'static Mutex<HashMap<String, Opened>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Opened>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_error_cell() -> &'static Mutex<Option<String>> {
    static LAST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn note_error(message: &str) {
    if let Ok(mut guard) = last_error_cell().lock() {
        *guard = Some(message.to_string());
    }
}

/// Last GPU failure, if any. Cleared on a successful kernel run.
pub fn last_error() -> Option<String> {
    last_error_cell()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

fn clear_error() {
    if let Ok(mut guard) = last_error_cell().lock() {
        *guard = None;
    }
}

/// Forget every cached device and the adapter listing; the next kernel reopens
/// them. An eGPU plugged in after launch appears after this call.
pub fn invalidate() {
    if let Ok(mut map) = cache().lock() {
        map.clear();
    }
    if let Ok(mut guard) = adapter_labels_cache().lock() {
        *guard = None;
    }
}

fn cached_set(key: &str) -> Option<Opened> {
    let map = cache().lock().ok()?;
    map.get(key).map(|entry| match entry {
        Ok(contexts) => Ok(contexts.clone()),
        Err(error) => Err(error.clone()),
    })
}

fn store_set(key: &str, value: &Opened) {
    if let Ok(mut map) = cache().lock() {
        let stored = match value {
            Ok(contexts) => Ok(contexts.clone()),
            Err(error) => Err(error.clone()),
        };
        map.insert(key.to_string(), stored);
    }
}

/// Open one adapter (or the first available when `adapter` is empty).
pub fn open(adapter: &str, all: bool) -> Opened {
    if all {
        return open_set(&[]);
    }
    if adapter.trim().is_empty() {
        return open_first();
    }
    open_set(&[adapter.to_string()])
}

/// Open the first adapter wgpu reports.
///
/// The kernel path asks for this on every call when the preference names no
/// adapter, so it must not enumerate the adapters again: `open_uncached` does
/// the enumeration once and the result is cached under a fixed key.
fn open_first() -> Opened {
    const KEY: &str = "<premier adaptateur>";
    if let Some(hit) = cached_set(KEY) {
        return hit;
    }
    let result = open_uncached(&[], true);
    store_set(KEY, &result);
    if let Err(error) = &result {
        note_error(error);
    }
    result
}

/// Open every requested adapter; an empty list means "every detected adapter".
pub fn open_set(requested: &[String]) -> Opened {
    let cache_key = requested.join(",");
    if let Some(hit) = cached_set(&cache_key) {
        return hit;
    }
    let result = open_uncached(requested, false);
    store_set(&cache_key, &result);
    if let Err(error) = &result {
        note_error(error);
    }
    result
}

fn open_uncached(requested: &[String], first_only: bool) -> Opened {
    let available: Vec<(wgpu::Adapter, AdapterChoice)> = adapters();
    if available.is_empty() {
        return Err("aucun adaptateur GPU de calcul détecté".to_string());
    }
    let mut selected = vec![false; available.len()];
    if requested.is_empty() {
        // One device per physical GPU: prefer the first backend that exposes it
        // so a GPU listed by both Vulkan and DX12 is not used twice.
        let mut names: Vec<String> = Vec::new();
        for (index, (_, choice)) in available.iter().enumerate() {
            if !names.iter().any(|name| name == &choice.name) {
                names.push(choice.name.clone());
                selected[index] = true;
                if first_only {
                    break;
                }
            }
        }
    } else {
        for wanted in requested {
            let found = available.iter().position(|(_, choice)| {
                choice.key.eq_ignore_ascii_case(wanted) || choice.name.eq_ignore_ascii_case(wanted)
            });
            match found {
                Some(index) => selected[index] = true,
                None => {
                    let names = available
                        .iter()
                        .map(|(_, choice)| choice.label())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(format!(
                        "adaptateur « {wanted} » introuvable (détectés : {names})"
                    ));
                }
            }
        }
    }
    let mut contexts = Vec::new();
    let mut last = String::new();
    for (index, (adapter, choice)) in available.into_iter().enumerate() {
        if !selected[index] {
            continue;
        }
        match GpuContext::open(adapter, choice) {
            Ok(context) => contexts.push(Arc::new(context)),
            Err(error) => last = error,
        }
    }
    if contexts.is_empty() {
        return Err(if last.is_empty() {
            "aucun périphérique GPU disponible".to_string()
        } else {
            last
        });
    }
    Ok(contexts)
}

// ---------------------------------------------------------------------------
// Kernels (multi-device aware)
// ---------------------------------------------------------------------------

fn contexts_for(state: &BackendState) -> Result<Vec<Arc<GpuContext>>, String> {
    match state.backend {
        vf_core::compute::ComputeBackend::Cpu => Err("calcul CPU sélectionné".to_string()),
        vf_core::compute::ComputeBackend::Gpu => open(&state.adapter, false),
        vf_core::compute::ComputeBackend::GpuAll => open_set(&state.adapters),
    }
}

/// Split `len` items between `contexts`, run `job` on each device in its own
/// thread and assemble the contiguous results.
fn run_split<F>(contexts: &[Arc<GpuContext>], len: usize, job: F) -> Result<Vec<Vec3>, String>
where
    F: Fn(&GpuContext, Range<usize>) -> Result<Vec<Vec3>, String> + Sync,
{
    if len == 0 {
        return Ok(Vec::new());
    }
    if contexts.len() <= 1 {
        return job(&contexts[0], 0..len);
    }
    let parts = contexts.len().min(len);
    let chunk = len.div_ceil(parts);
    let ranges: Vec<Range<usize>> = (0..parts)
        .map(|index| {
            let start = (index * chunk).min(len);
            start..((start + chunk).min(len))
        })
        .collect();
    let results: Vec<Result<Vec<Vec3>, String>> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(parts);
        for index in 0..parts {
            let context = &contexts[index];
            let range = ranges[index].clone();
            let job = &job;
            handles.push(scope.spawn(move || job(context, range)));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("thread GPU interrompu".to_string()))
            })
            .collect()
    });
    let mut out = vec![Vec3::ZERO; len];
    for (index, result) in results.into_iter().enumerate() {
        let values = result?;
        let start = ranges[index].start;
        for (offset, value) in values.into_iter().enumerate() {
            if start + offset < len {
                out[start + offset] = value;
            }
        }
    }
    Ok(out)
}

pub fn normals(
    state: &BackendState,
    positions: &[Vec3],
    graph: &FaceGraph,
    corners: &VertexCorners,
) -> Result<Vec<Vec3>, String> {
    let contexts = contexts_for(state)?;
    let result = run_split(&contexts, positions.len(), |context, range| {
        context.vertex_normals(positions, graph, corners, range)
    });
    match &result {
        Ok(_) => clear_error(),
        Err(error) => note_error(error),
    }
    result
}

pub fn area_normals(
    state: &BackendState,
    positions: &[Vec3],
    graph: &FaceGraph,
    corners: &VertexCorners,
) -> Result<Vec<Vec3>, String> {
    let contexts = contexts_for(state)?;
    let result = run_split(&contexts, positions.len(), |context, range| {
        context.area_normals(positions, graph, corners, range)
    });
    match &result {
        Ok(_) => clear_error(),
        Err(error) => note_error(error),
    }
    result
}

pub fn smooth(
    state: &BackendState,
    positions: &[Vec3],
    neighbours: &Neighbours,
    iterations: u32,
    lambda: f32,
    mu: f32,
) -> Result<Vec<Vec3>, String> {
    let contexts = contexts_for(state)?;
    let mut passes = Vec::with_capacity(iterations as usize * 2);
    for _ in 0..iterations {
        passes.push(lambda);
        passes.push(mu);
    }
    if passes.is_empty() {
        return Ok(positions.to_vec());
    }
    let result = run_split(&contexts, positions.len(), |context, range| {
        context.smooth(positions, neighbours, &passes, range)
    });
    match &result {
        Ok(_) => clear_error(),
        Err(error) => note_error(error),
    }
    result
}

pub fn displace(
    state: &BackendState,
    positions: &[Vec3],
    normals: &[Vec3],
    amounts: &[f32],
) -> Result<Vec<Vec3>, String> {
    let contexts = contexts_for(state)?;
    let count = positions.len().min(normals.len()).min(amounts.len());
    let result = run_split(&contexts, count, |context, range| {
        context.displace(positions, normals, amounts, range)
    });
    match &result {
        Ok(_) => clear_error(),
        Err(error) => note_error(error),
    }
    result
}

pub fn transform(
    state: &BackendState,
    positions: &[Vec3],
    matrix: Mat4,
) -> Result<Vec<Vec3>, String> {
    let contexts = contexts_for(state)?;
    let result = run_split(&contexts, positions.len(), |context, range| {
        context.transform(positions, matrix, range)
    });
    match &result {
        Ok(_) => clear_error(),
        Err(error) => note_error(error),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapters_are_enumerable() {
        for (key, label) in detected() {
            println!("{key} -> {label}");
        }
    }

    #[test]
    fn normals_match_the_cpu_reference() {
        // A flat square in the XY plane: every vertex normal points at +Z.
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let graph = FaceGraph::from_faces([&[0u32, 1, 2][..], &[0, 2, 3][..]]);
        let corners = VertexCorners::build(4, &graph);
        let Ok(contexts) = open_set(&[]) else {
            eprintln!("aucun adaptateur GPU, test ignoré");
            return;
        };
        let context = &contexts[0];
        let values = context
            .vertex_normals(&positions, &graph, &corners, 0..4)
            .expect("normales GPU");
        for normal in &values {
            assert!((normal.z - 1.0).abs() < 1e-5, "{normal:?}");
        }
    }

    #[test]
    fn smoothing_round_trips_on_the_gpu() {
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.5, 0.5, 4.0),
        ];
        let graph = FaceGraph::from_faces([
            &[0u32, 1, 4][..],
            &[1, 2, 4][..],
            &[2, 3, 4][..],
            &[3, 0, 4][..],
        ]);
        let neighbours = Neighbours::build(5, &graph);
        let Ok(contexts) = open_set(&[]) else {
            eprintln!("aucun adaptateur GPU, test ignoré");
            return;
        };
        let context = &contexts[0];
        let values = context
            .smooth(&positions, &neighbours, &[0.5, -0.53], 0..5)
            .expect("lissage GPU");
        // The apex must come down towards the base.
        assert!(values[4].z < positions[4].z);
        assert!(values.iter().all(|value| value.is_finite()));
    }
}
