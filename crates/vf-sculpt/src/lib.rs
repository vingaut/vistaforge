//! VistaForge 3D mesh workshop.
//!
//! The heightmap pipeline produces a surface; this crate turns that surface
//! into a real, editable polygonal mesh and gives it everything a heightfield
//! cannot do: overhangs, true fractures, impacts, melting, voxel booleans,
//! metaballs, debris physics and adaptive remeshing in triangles or quads.
//!
//! # Layers
//!
//! * [`mesh::Mesh`] — n-gon mesh with normals, UVs, colours and materials.
//! * [`bvh::Bvh`] — ray casts, closest points and radius queries.
//! * [`voxel`] — signed-distance grid, surface-nets polygonisation, booleans.
//! * [`remesh`] — QEM decimation, adaptive refinement, isotropic remeshing and
//!   quad pairing, all projecting back onto the source surface.
//! * [`sculpt`] — hand tools: hammer, chisels, file, clay, undercut, strata,
//!   melt, impacts and fractures.
//! * [`fracture`] — crack networks and true Voronoi cell fracture.
//! * [`metaball`] — implicit blob fields blended with the mesh.
//! * [`physics`] — deterministic debris simulation for rockfall and damage.
//! * [`cliff`] — procedural overhang/wall shaping guided by the reference
//!   photographs of layered sea cliffs and sandstone walls.
//! * [`render`] — CPU viewport renderer with wireframe, masks and view modes.
//! * [`doc::SculptDocument`] — the editable document with history and stats.
//!
//! Everything is deterministic: the same seed and the same operation list give
//! the same mesh on every machine and with any thread count.

pub mod bvh;
pub mod cliff;
pub mod doc;
pub mod fracture;
pub mod io;
pub mod math;
pub mod mesh;
pub mod metaball;
pub mod noise;
pub mod physics;
pub mod remesh;
pub mod render;
pub mod rock;
pub mod sculpt;
pub mod terrain;
pub mod voxel;

pub use bvh::{Bvh, ClosestHit, RayHit};
pub use cliff::{CliffSettings, CliffStyle};
pub use doc::{QualityLevel, SculptDocument, SculptStats};
pub use fracture::{FracturePattern, FracturePiece, FractureResult, FractureSettings};
pub use io::{MeshMetadata, load_mesh, save_mesh};
pub use math::{Aabb, Falloff, Ray};
pub use mesh::{Mesh, MeshStats, RenderMesh};
pub use metaball::{FusionSettings, MetaField};
pub use noise::{NoiseKind, NoiseParams, RockSurface};
pub use physics::{PhysicsSettings, PhysicsWorld};
pub use remesh::{RemeshMode, RemeshReport, RemeshSettings};
pub use render::{Camera, RenderSettings, ShadingMode};
pub use rock::{RockDetailSettings, RockProfile};
pub use sculpt::{BrushSettings, ToolKind};
pub use terrain::TerrainMeshSettings;

/// Version of the `.vfm` mesh file this crate writes.
pub const MESH_FORMAT_VERSION: u32 = 1;

/// Error type shared by the file readers/writers and the recipe runner.
#[derive(Debug, thiserror::Error)]
pub enum SculptError {
    #[error("mesh is empty")]
    EmptyMesh,
    #[error("invalid mesh: {0}")]
    InvalidMesh(String),
    #[error("operation not supported: {0}")]
    Unsupported(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("terrain mesh error: {0}")]
    Terrain(String),
}

pub type Result<T> = std::result::Result<T, SculptError>;
