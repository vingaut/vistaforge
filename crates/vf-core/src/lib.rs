//! Core data model shared by all VistaForge crates.
//!
//! Conventions:
//! * world space is right-handed, Z-up, X east, Y north, metres;
//! * heightfields are row-major, south row first, north last;
//! * seeds are explicit and derived by name, never global;
//! * serialisable scene settings live here, while behaviour lives in
//!   `vf-scene`, `vf-terrain`, `vf-procgen` and `vf-render`.

pub mod color;
pub mod compute;
pub mod detail;
pub mod error;
pub mod flight;
pub mod geology;
pub mod heightfield;
pub mod ids;
pub mod modifiers;
pub mod project;
pub mod rig;
pub mod scene;
pub mod seed;
pub mod stats;
pub mod terrain_params;
pub mod world;

pub use color::Rgb;
pub use compute::{ComputeBackend, ComputeSettings, GpuPower};
pub use detail::{
    CraterField, DETAIL_PRESETS, DetailSettings, Exfoliation, Grooves, Ridges, Vesicles,
};
pub use error::{Error, Result};
pub use flight::{FLIGHT_PRESETS, FlightPattern, FlightSettings};
pub use geology::{
    Dissolution, GEOLOGY_PRESETS, GeologySettings, Gullies, JointPattern, Joints, Strata,
};
pub use heightfield::{Bounds, HeightField, HeightFieldError};
pub use ids::{ChunkId, LayerId, ProjectId};
pub use modifiers::{Modifier, ModifierKind, ModifierStack, ParamSpec};
pub use project::{LegacyMeta, Project, ProjectMeta, SCHEMA_VERSION, TerrainMeta};
pub use rig::{
    AircraftPreset, CameraLink, CameraRig, Constraints, KeyAngles, MotionProfile, OpticalRig,
    Path3D, PathKey, PursuitAngles, PursuitKey, TargetMode, TargetRig,
};
pub use scene::{
    Atmosphere, Camera, CloudSettings, Materials, Palette, Quality, ROCK_PRESETS, RenderMode,
    RenderSettings, Sun, TerrainStyle, Tonemap, WaterSettings,
};
pub use seed::{Rng, Seed};
pub use stats::TerrainStats;
pub use terrain_params::{ClassicParams, DemSource, ErosionParams, ModernParams, TerrainSource};
pub use world::{TileVariation, WorldMode, WorldSettings};
