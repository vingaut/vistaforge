//! Selectable compute backend for VistaForge geometry kernels.
//!
//! Geometric transformations (normals, smoothing, rock detail displacement and
//! rigid transforms) are the expensive part of mesh editing. This crate routes
//! them through one of two backends chosen in the preferences:
//!
//! - **CPU**: deterministic, chunked rayon kernels on the shared
//!   [`vf_core::compute`] pool, so the thread count selected in the UI applies
//!   to geometry work and not only to rendering.
//! - **GPU**: wgpu compute pipelines, on one selected adapter or spread across
//!   every adapter when "all GPUs" is requested. A device that cannot be
//!   opened falls back to the CPU path transparently.
//!
//! Every kernel is deterministic for a given backend: the CPU path chunks work
//! in fixed blocks and reduces in a fixed order, and the GPU path assigns one
//! vertex per invocation with a fixed iteration order. The CPU path is
//! bit-identical to the serial reference implementations it replaces; the GPU
//! path runs the same formulas but may differ in the last bit because drivers
//! are free to contract multiplies and additions.

pub mod backend;
pub mod kernels;
pub mod mesh_graph;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use backend::{
    BackendState, ComputeBackend, ComputeStatus, backend_state, configure, detected_adapters,
    invalidate_gpu, set_backend, status, status_for,
};
pub use kernels::{
    BackendUsed, Outcome, area_normals, area_normals_cpu, displace, gpu_selected, normals,
    normals_cpu, smooth, smooth_cpu, transform,
};
pub use mesh_graph::{FaceGraph, Neighbours, VertexCorners, VertexFaces};

/// Last GPU failure, if the GPU path fell back to the CPU.
pub fn last_error() -> Option<String> {
    #[cfg(feature = "gpu")]
    {
        gpu::last_error()
    }
    #[cfg(not(feature = "gpu"))]
    {
        None
    }
}

#[cfg(test)]
mod tests;
