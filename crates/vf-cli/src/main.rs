use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;
use vf_core::{
    ClassicParams, DemSource, ErosionParams, FlightSettings, GeologySettings, HeightField,
    LegacyMeta, ModifierKind, Palette, Project, Quality, Seed, TerrainSource,
};
use vf_io::DemFile;
use vf_render::{RenderOptions, render_with_options};
use vf_terrain::{HillshadeParams, ResampleFilter};

#[derive(Debug, Parser)]
#[command(
    name = "vistaforge",
    version,
    about = "VistaForge — modern landscape generator and editor",
    long_about = "Generate, edit and render landscapes from procedural terrain or real DEMs."
)]
struct Cli {
    /// Print more detail about each operation.
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect, convert and preview legacy DEM files.
    Dem {
        #[command(subcommand)]
        command: DemCommand,
    },
    /// Generate, import, inspect and edit terrain projects.
    Terrain {
        #[command(subcommand)]
        command: TerrainCommand,
    },
    /// Apply scene, lighting and palette presets.
    Scene {
        #[command(subcommand)]
        command: SceneCommand,
    },
    /// Render a .vfproj project to PNG.
    Render(RenderArgs),
    /// Fly the camera over a project and render stills or a frame sequence.
    Anim {
        #[command(subcommand)]
        command: AnimCommand,
    },
    /// List available generation and scene presets.
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    /// Project metadata helpers.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Export the scene to Blender (GLB + textures + import script).
    Blender {
        #[command(subcommand)]
        command: BlenderCommand,
    },
    /// Inspect and author the LightWave-style camera rig.
    Rig {
        #[command(subcommand)]
        command: RigCommand,
    },
}

#[derive(Debug, Subcommand)]
enum RigCommand {
    /// Print the rig path, optics and motion profile.
    Info {
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Apply an aircraft preset and a terrain-fitted path.
    Preset {
        project: PathBuf,
        /// drone, bird, helicopter, airship, fighter or manual.
        preset: String,
        /// orbit, ridge or dive.
        #[arg(long, default_value = "orbit")]
        path: String,
        #[arg(long)]
        duration: Option<f32>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Append a key to the point-of-view path.
    Key {
        project: PathBuf,
        #[arg(long)]
        time: f32,
        #[arg(long)]
        x: f64,
        #[arg(long)]
        y: f64,
        #[arg(long)]
        z: f64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Export the camera/target animation only (.vfcam, .glb or .csv).
    Export {
        project: PathBuf,
        output: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Import a camera/target animation without changing the terrain.
    Import {
        project: PathBuf,
        animation: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum BlenderCommand {
    /// Write scene.glb, scene.json, heightmap and a README.
    Export {
        project: PathBuf,
        out: PathBuf,
        /// Mesh density (defaults to the project render subdivision).
        #[arg(long)]
        subdivision: Option<u32>,
        #[arg(long)]
        no_water: bool,
        #[arg(long)]
        no_camera: bool,
        #[arg(long)]
        no_sun: bool,
        #[arg(long)]
        json: bool,
    },
    /// Export, then import and render in Blender (Cycles).
    Render {
        project: PathBuf,
        image: PathBuf,
        /// Directory for the intermediate GLB/sidecar.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Blender executable (defaults to `blender` in PATH).
        #[arg(long)]
        blender: Option<PathBuf>,
        /// Import script path.
        #[arg(long)]
        script: Option<PathBuf>,
        #[arg(long, default_value_t = 32)]
        samples: u32,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        /// Try OptiX/CUDA instead of CPU.
        #[arg(long)]
        gpu: bool,
        /// Run Blender Smart UV Project on the imported mesh.
        #[arg(long)]
        smart_uv: bool,
        /// Apply a voxel Remesh modifier in Blender (destructive).
        #[arg(long)]
        remesh: bool,
        /// Save the configured .blend before rendering.
        #[arg(long)]
        save_blend: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum DemCommand {
    /// Print header, dimensions and elevation statistics.
    Info {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Re-encode a DEM, or export raw i16 / ASCII samples.
    Convert {
        input: PathBuf,
        output: PathBuf,
        /// Write raw little-endian i16 samples (legacy binary format).
        #[arg(long, conflicts_with = "ascii")]
        raw: bool,
        /// Write an ESRI ASCII Grid instead of a DEM.
        #[arg(long)]
        ascii: bool,
    },
    /// Write a shaded-relief PNG preview.
    Preview {
        input: PathBuf,
        output: PathBuf,
        /// Output size in pixels. 0 keeps the DEM size.
        #[arg(long, default_value_t = 1024)]
        size: u32,
        /// Sun azimuth in compass degrees (0 = south, 90 = west).
        #[arg(long, default_value_t = 315.0)]
        azimuth: f32,
        /// Sun altitude above the horizon, in degrees.
        #[arg(long, default_value_t = 45.0)]
        altitude: f32,
        /// Vertical exaggeration used for shading.
        #[arg(long, default_value_t = 2.0)]
        z_factor: f32,
    },
    /// Parse, re-encode and re-parse a DEM, then compare the samples.
    Roundtrip { path: PathBuf },
}

#[derive(Debug, Subcommand)]
enum TerrainCommand {
    /// Generate a Classic or Modern fractal terrain and save a project.
    Generate(GenerateArgs),
    /// Import a GeoTIFF DEM, ESRI ASCII grid, legacy DEM or heightmap image.
    Import(ImportArgs),
    /// Export the current terrain to GeoTIFF, ASCII, DEM, PNG or JPEG.
    Export {
        project: PathBuf,
        output: PathBuf,
        #[arg(long)]
        nodata: Option<f32>,
        #[arg(long)]
        json: bool,
    },
    /// Print project and terrain statistics.
    Info {
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Apply a rock preset to the current heightfield (useful for DEMs).
    Rock {
        project: PathBuf,
        /// granite, basalt, limestone, sandstone, schist, chalk or volcanic.
        #[arg(long)]
        preset: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Resample the terrain to a new size.
    Resample {
        project: PathBuf,
        #[arg(long)]
        width: usize,
        #[arg(long)]
        height: Option<usize>,
        #[arg(long, value_enum, default_value_t = FilterArg::Bicubic)]
        filter: FilterArg,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Smooth the terrain with a Gaussian filter.
    Smooth {
        project: PathBuf,
        #[arg(long, default_value_t = 2)]
        radius: usize,
        #[arg(long, default_value_t = 1)]
        iterations: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Apply a manual brush stamp or stroke and save the project.
    Sculpt(SculptArgs),
    /// Set or apply a tiled / seamless world composition.
    World(WorldArgs),
    /// List, add, edit, reorder, remove or bake the modifier stack.
    Modifier {
        #[command(subcommand)]
        command: ModifierCommand,
    },
}

#[derive(Debug, Args)]
struct SculptArgs {
    project: PathBuf,
    /// Stamp centre, world metres.
    #[arg(long)]
    x: f64,
    #[arg(long)]
    y: f64,
    /// End of a straight stroke; omit for a single stamp.
    #[arg(long)]
    x2: Option<f64>,
    #[arg(long)]
    y2: Option<f64>,
    /// raise, lower, smooth, flatten, terrace, ridge, groove, crater, noise,
    /// warp or clone.
    #[arg(long, default_value = "raise")]
    mode: String,
    /// circle, square, diamond, ring, star or polygon.
    #[arg(long, default_value = "circle")]
    shape: String,
    /// constant, linear, smooth, gaussian or plateau.
    #[arg(long, default_value = "smooth")]
    falloff: String,
    #[arg(long, default_value_t = 80.0)]
    radius: f32,
    #[arg(long, default_value_t = 8.0)]
    strength: f32,
    #[arg(long, default_value_t = 0.25)]
    roughness: f32,
    /// Edge hardness, 0 = soft, 1 = hard.
    #[arg(long, default_value_t = 0.35)]
    hardness: f32,
    #[arg(long, default_value_t = 4)]
    aa: u32,
    #[arg(long, default_value_t = 1.0)]
    aspect: f32,
    #[arg(long, default_value_t = 0.0)]
    angle: f32,
    #[arg(long, default_value_t = 25.0)]
    step: f32,
    /// Stratum dip for terraces, degrees.
    #[arg(long, default_value_t = 0.0)]
    dip: f32,
    /// Down-dip azimuth, degrees (0 = +X, 90 = +Y).
    #[arg(long, default_value_t = 0.0)]
    dip_azimuth: f32,
    /// Fraction of each stratum that is a flat bench (0.3..0.99).
    #[arg(long, default_value_t = 0.82)]
    bench_ratio: f32,
    /// Relative thickness variation between benches (0..1).
    #[arg(long, default_value_t = 0.25)]
    bench_variation: f32,
    /// Riser sharpness (0 = wide slope, 1 = near-vertical step).
    #[arg(long, default_value_t = 0.7)]
    riser_sharpness: f32,
    /// Columnar joint spacing in metres (0 disables joints).
    #[arg(long, default_value_t = 0.0)]
    column_spacing: f32,
    /// Depth of the columnar grooves, metres.
    #[arg(long, default_value_t = 2.0)]
    column_depth: f32,
    /// Irregularity of the column spacing (0..1).
    #[arg(long, default_value_t = 0.35)]
    column_jitter: f32,
    #[arg(long)]
    invert: bool,
    /// none, x, y, xy or radial.
    #[arg(long, default_value = "none")]
    symmetry: String,
    #[arg(long, default_value_t = 6)]
    radial: u32,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct WorldArgs {
    project: PathBuf,
    /// single, tiled or seamless.
    #[arg(long, default_value = "tiled")]
    mode: String,
    #[arg(long, default_value_t = 2)]
    tiles_x: u32,
    #[arg(long, default_value_t = 2)]
    tiles_y: u32,
    /// Edge-correction band, samples.
    #[arg(long, default_value_t = 48)]
    blend: u32,
    /// Disable flips/rotations of the instances.
    #[arg(long)]
    no_variation: bool,
    /// Smooth world-scale height variation, metres.
    #[arg(long = "macro", default_value_t = 0.0)]
    macro_variation: f32,
    /// Wavelength of the macro variation, metres.
    #[arg(long, default_value_t = 2400.0)]
    macro_scale: f32,
    /// Compose the heightfield now and write the result.
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ModifierCommand {
    /// Print the stack and the parameters of each entry.
    List {
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Append a modifier. Parameters use `--param key=value`.
    Add {
        project: PathBuf,
        /// Modifier name, e.g. displace, warp, terrace, crater, erosion.
        kind: String,
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Change parameters or enable/disable one entry.
    Set {
        project: PathBuf,
        index: usize,
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Remove one entry.
    Remove {
        project: PathBuf,
        index: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Move one entry up or down in the stack.
    Move {
        project: PathBuf,
        index: usize,
        #[arg(long, conflicts_with = "down")]
        up: bool,
        #[arg(long)]
        down: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Remove every entry.
    Clear {
        project: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Apply the stack to the stored heightfield and clear it.
    Bake {
        project: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct GenerateArgs {
    #[arg(long, value_enum, default_value_t = ModeArg::Classic)]
    mode: ModeArg,
    /// Grid size in samples (258, 514, 1026, 2050, up to 16384).
    #[arg(long, default_value_t = 1026)]
    size: u32,
    /// Seed. A random seed is chosen and printed when omitted.
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long)]
    name: Option<String>,
    /// Output project path. Defaults to <name>.vfproj.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Modern preset: alpine, desert, volcanic, coastal, canyon, lunar, island.
    #[arg(long, default_value = "alpine")]
    preset: String,
    /// Classic fractal dimension (0..200).
    #[arg(long)]
    dimension: Option<f32>,
    /// Classic divisor: 1, 2, 4 or 8.
    #[arg(long)]
    divisor: Option<u8>,
    #[arg(long)]
    island: bool,
    #[arg(long)]
    vertical_scale: Option<f32>,
    #[arg(long)]
    ridge: Option<f32>,
    #[arg(long)]
    warp: Option<f32>,
    #[arg(long)]
    continent: Option<f32>,
    /// Geological preset: granite, basalt, limestone, sandstone, schist,
    /// chalk or volcanic. Applied to the heightfield before erosion.
    #[arg(long)]
    rock: Option<String>,
    /// Colour palette (alpine, desert, volcanic, coastal, canyon, lunar, island, dusk).
    #[arg(long)]
    palette: Option<String>,
    /// Rock-detail profile (granite, basalt, limestone, sandstone, schist,
    /// chalk, volcanic, canyon, patagonia). Defaults to the --rock profile.
    #[arg(long)]
    detail_profile: Option<String>,
    /// Disable the continuous detail layer for this project.
    #[arg(long)]
    no_detail: bool,
    /// Modern edge falloff, 0 = a flat, tileable border.
    #[arg(long)]
    border_falloff: Option<f32>,
    /// Apply hydraulic + thermal erosion.
    #[arg(long)]
    erode: bool,
    #[arg(long)]
    droplets: Option<u32>,
    /// Render a preview PNG immediately after generation.
    #[arg(long)]
    render: Option<PathBuf>,
    #[arg(long, default_value_t = 1280)]
    render_width: u32,
    #[arg(long, default_value_t = 720)]
    render_height: u32,
}

#[derive(Debug, Subcommand)]
enum AnimCommand {
    /// Render one frame of the flight at a given time.
    Still(AnimStillArgs),
    /// Render a PNG sequence of the flight.
    Render(AnimRenderArgs),
}

#[derive(Debug, Args)]
struct AnimStillArgs {
    project: PathBuf,
    output: PathBuf,
    /// Flight preset: cinelifter, freestyle, racing, helicopter, cinematic, dive.
    #[arg(long)]
    preset: Option<String>,
    /// Clip time in seconds.
    #[arg(long, default_value_t = 2.0)]
    time: f64,
    #[arg(long)]
    width: Option<u32>,
    #[arg(long)]
    height: Option<u32>,
    #[arg(long, value_enum)]
    quality: Option<QualityArg>,
    /// fast, refined or photoreal.
    #[arg(long)]
    mode: Option<String>,
    #[arg(long)]
    exposure: Option<f32>,
}

#[derive(Debug, Args)]
struct AnimRenderArgs {
    project: PathBuf,
    /// Output directory for frame_00000.png files.
    out: PathBuf,
    /// Flight preset: cinelifter, freestyle, racing, helicopter, cinematic, dive.
    #[arg(long)]
    preset: Option<String>,
    #[arg(long)]
    seconds: Option<f32>,
    #[arg(long)]
    fps: Option<u32>,
    #[arg(long)]
    width: Option<u32>,
    #[arg(long)]
    height: Option<u32>,
    #[arg(long, value_enum)]
    quality: Option<QualityArg>,
    /// fast, refined or photoreal.
    #[arg(long)]
    mode: Option<String>,
    /// Render every Nth frame (1 = all frames).
    #[arg(long, default_value_t = 1)]
    stride: usize,
    /// Shutter angle in degrees: 180 gives half-frame motion blur, 0 disables.
    #[arg(long, default_value_t = 0.0)]
    shutter: f32,
    /// Number of sub-frames accumulated inside the shutter window.
    #[arg(long, default_value_t = 4)]
    blur_samples: u32,
    /// Encode an MP4 with ffmpeg once the frames are written.
    #[arg(long)]
    video: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ImportArgs {
    input: PathBuf,
    /// Output project path. Defaults to <input stem>.vfproj.
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    /// Horizontal sample spacing in metres (default 30 for DEMs; ASCII grids
    /// keep their header cellsize unless this is set).
    #[arg(long)]
    spacing: Option<f32>,
    /// Force the legacy ESRI ASCII reader instead of format detection.
    #[arg(long)]
    ascii: bool,
    /// Minimum elevation for normalised heightmaps (PNG/JPEG); 0 with
    /// --max-height 0 means "use the JSON sidecar, else 0..1000".
    #[arg(long, default_value_t = 0.0)]
    min_height: f32,
    /// Maximum elevation for normalised heightmaps (PNG/JPEG).
    #[arg(long, default_value_t = 0.0)]
    max_height: f32,
    /// Keep image rows north-first instead of flipping to VistaForge.
    #[arg(long)]
    no_flip_y: bool,
    /// Invert the height convention: dark pixels become high ground.
    #[arg(long)]
    invert_height: bool,
    /// Render a preview PNG immediately after import.
    #[arg(long)]
    render: Option<PathBuf>,
    #[arg(long, default_value_t = 1280)]
    render_width: u32,
    #[arg(long, default_value_t = 720)]
    render_height: u32,
}

#[derive(Debug, Subcommand)]
enum SceneCommand {
    /// Apply a scene preset and reframe the camera.
    Preset {
        project: PathBuf,
        /// alpine, sunset, desert, volcanic, lunar, coastal.
        name: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_reframe: bool,
    },
}

#[derive(Debug, Args)]
struct RenderArgs {
    project: PathBuf,
    output: PathBuf,
    #[arg(long)]
    width: Option<u32>,
    #[arg(long)]
    height: Option<u32>,
    #[arg(long, value_enum)]
    quality: Option<QualityArg>,
    /// fast, refined or photoreal.
    #[arg(long)]
    mode: Option<String>,
    #[arg(long)]
    ssaa: Option<u8>,
    #[arg(long)]
    preview: bool,
    #[arg(long, conflicts_with = "no_shadows")]
    shadows: bool,
    #[arg(long)]
    no_shadows: bool,
    #[arg(long, conflicts_with = "no_ao")]
    ao: bool,
    #[arg(long)]
    no_ao: bool,
    /// Enable the local cliff mesh (on by default).
    #[arg(long, conflicts_with = "no_cliffs")]
    cliffs: bool,
    /// Disable the local cliff mesh.
    #[arg(long)]
    no_cliffs: bool,
    #[arg(long, conflicts_with = "no_clouds")]
    clouds: bool,
    #[arg(long)]
    no_clouds: bool,
    #[arg(long, conflicts_with = "no_water")]
    water: bool,
    #[arg(long)]
    no_water: bool,
    #[arg(long)]
    exposure: Option<f32>,
    /// Sun as "azimuth,declination" (compass azimuth: 0 = south, 90 = west).
    #[arg(long)]
    sun: Option<String>,
    /// Camera position "x,y,z" in world metres.
    #[arg(long)]
    camera: Option<String>,
    /// Camera target "x,y,z" in world metres.
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    fov: Option<f32>,
    #[arg(long)]
    auto_frame: bool,
    /// Geometric subdivision of the terrain mesh (1..4).
    #[arg(long)]
    subdivision: Option<u8>,
    /// Disable the screen-adaptive block LOD.
    #[arg(long)]
    no_adaptive: bool,
    /// Enable the continuous rock detail layer.
    #[arg(long, conflicts_with = "no_detail")]
    detail: bool,
    /// Disable the continuous rock detail layer.
    #[arg(long)]
    no_detail: bool,
    /// Detail profile: granite, basalt, limestone, sandstone, schist, chalk,
    /// volcanic, canyon, patagonia or generic.
    #[arg(long)]
    detail_profile: Option<String>,
    /// Detail master strength (0..2).
    #[arg(long)]
    detail_strength: Option<f32>,
    /// Base detail wavelength, metres.
    #[arg(long)]
    detail_scale: Option<f32>,
    /// Peak-to-peak detail amplitude, metres.
    #[arg(long)]
    detail_amplitude: Option<f32>,
    /// High-density close-up patch centre "x,y" in world metres.
    #[arg(long)]
    detail_focus: Option<String>,
    /// Close-up patch size, metres.
    #[arg(long)]
    detail_focus_size: Option<f32>,
    /// Close-up patch vertex spacing, metres.
    #[arg(long)]
    detail_focus_spacing: Option<f32>,
    /// Legacy alias for the material bump strength (0 disables it).
    #[arg(long)]
    detail_normal: Option<f32>,
    /// Large-scale detail patchiness (0..1), breaks layer repetition.
    #[arg(long)]
    detail_heterogeneity: Option<f32>,
    /// Procedural rock preset: granite, basalt, limestone, sandstone,
    /// schist, chalk, volcanic or generic.
    #[arg(long)]
    rock_material: Option<String>,
    /// Material bump strength (0..2).
    #[arg(long)]
    bump: Option<f32>,
    /// Material bump wavelength, metres.
    #[arg(long)]
    bump_scale: Option<f32>,
    /// Material displacement, metres.
    #[arg(long)]
    displacement: Option<f32>,
    /// Custom texture path (PNG/JPG/TIFF) projected in world space.
    #[arg(long)]
    texture: Option<String>,
    /// World-space texture tile size, metres.
    #[arg(long)]
    texture_scale: Option<f32>,
    /// Texture bump strength (0..2).
    #[arg(long)]
    texture_bump: Option<f32>,
    /// Texture displacement, metres.
    #[arg(long)]
    texture_displacement: Option<f32>,
    /// Texture mix with the geological rock colour (0..1).
    #[arg(long)]
    texture_mix: Option<f32>,
    /// Disable the shape-adaptive mesh (use the regular grid).
    #[arg(long)]
    no_shape_adaptive: bool,
    /// Shape-adaptive geometric tolerance, metres.
    #[arg(long)]
    adaptive_tolerance: Option<f32>,
    /// Write quads in the Blender OBJ export.
    #[arg(long)]
    quads: bool,
    /// Undulate the cliff ladder (0..1) to break horizontal repetition.
    #[arg(long)]
    cliff_warp: Option<f32>,
    /// Wavelength of the cliff-ladder undulation, metres.
    #[arg(long)]
    cliff_warp_scale: Option<f32>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum PresetCommand {
    /// List terrain, scene and palette presets.
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Print project metadata and statistics.
    Info {
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Classic,
    Modern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FilterArg {
    Nearest,
    Bilinear,
    Bicubic,
}

impl From<FilterArg> for ResampleFilter {
    fn from(value: FilterArg) -> Self {
        match value {
            FilterArg::Nearest => ResampleFilter::Nearest,
            FilterArg::Bilinear => ResampleFilter::Bilinear,
            FilterArg::Bicubic => ResampleFilter::Bicubic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum QualityArg {
    Low,
    Medium,
    High,
    Ultra,
}

impl From<QualityArg> for Quality {
    fn from(value: QualityArg) -> Self {
        match value {
            QualityArg::Low => Quality::Low,
            QualityArg::Medium => Quality::Medium,
            QualityArg::High => Quality::High,
            QualityArg::Ultra => Quality::Ultra,
        }
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    if cli.verbose {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();
    }
    match run(cli.command) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Dem { command } => run_dem(command),
        Command::Terrain { command } => run_terrain(command),
        Command::Scene { command } => run_scene(command),
        Command::Render(args) => run_render(args),
        Command::Anim { command } => run_anim(command),
        Command::Preset { command } => run_preset(command),
        Command::Project { command } => match command {
            ProjectCommand::Info { project, json } => print_project_info(&project, json),
        },
        Command::Blender { command } => run_blender(command),
        Command::Rig { command } => run_rig(command),
    }
}

fn run_dem(command: DemCommand) -> Result<()> {
    match command {
        DemCommand::Info { path, json } => {
            let dem = read_dem(&path)?;
            let (width, height) = dem.header.effective_dimensions();
            let (min, max, mean) = dem.min_max_mean();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "file": path,
                        "title": dem.header.title,
                        "comment": dem.header.comment,
                        "version": dem.header.version,
                        "flags": dem.header.flags,
                        "width": width,
                        "height": height,
                        "samples": dem.heights.len(),
                        "min": min,
                        "max": max,
                        "mean": mean,
                        "palette_entries": dem.palette.len(),
                        "session_bytes": dem.session.len(),
                    }))?
                );
            } else {
                println!("file:        {}", path.display());
                println!("title:       {:?}", dem.header.title);
                println!("comment:     {:?}", dem.header.comment);
                println!("version:     {}", dem.header.version);
                println!("flags:       0x{:08X}", dem.header.flags);
                println!(
                    "dimensions:  {}x{} (header {}x{})",
                    width, height, dem.header.width, dem.header.height
                );
                println!("samples:     {}", dem.heights.len());
                println!("elevation:   min={min} m  max={max} m  mean={mean:.2} m");
                println!("palette:     {} entries", dem.palette.len());
                println!("session:     {} bytes (opaque)", dem.session.len());
            }
        }
        DemCommand::Convert {
            input,
            output,
            raw,
            ascii,
        } => {
            let dem = read_dem(&input)?;
            if raw {
                let mut bytes = Vec::with_capacity(dem.heights.len() * 2);
                for value in &dem.heights {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                std::fs::write(&output, bytes)
                    .with_context(|| format!("cannot write {}", output.display()))?;
            } else if ascii {
                let hf = dem
                    .to_heightfield(30.0)
                    .context("cannot convert DEM to heightfield")?;
                vf_io::ascii::write_ascii_grid(&output, &hf, -9999.0)?;
            } else {
                let bytes = dem.write()?;
                std::fs::write(&output, bytes)
                    .with_context(|| format!("cannot write {}", output.display()))?;
            }
            println!(
                "wrote {} ({} bytes)",
                output.display(),
                std::fs::metadata(&output)?.len()
            );
        }
        DemCommand::Preview {
            input,
            output,
            size,
            azimuth,
            altitude,
            z_factor,
        } => {
            let dem = read_dem(&input)?;
            let hf = dem
                .to_heightfield(30.0)
                .context("cannot convert DEM to heightfield")?;
            let image = vf_terrain::hillshade(
                &hf,
                HillshadeParams {
                    azimuth_deg_from_south: azimuth,
                    declination_deg: altitude,
                    vertical_exaggeration: z_factor,
                    ambient: 0.12,
                },
            );
            let (w, h) = (image.width as u32, image.height as u32);
            let out = if size > 0 && size != w {
                let gray = image::GrayImage::from_raw(w, h, image.pixels.clone())
                    .context("invalid hillshade buffer")?;
                let resized = image::imageops::resize(
                    &gray,
                    size,
                    size,
                    image::imageops::FilterType::Lanczos3,
                );
                let (rw, rh) = resized.dimensions();
                vf_io::image_io::save_png_gray(&output, rw, rh, resized.as_raw())?;
                (rw, rh)
            } else {
                vf_io::image_io::save_png_gray(&output, w, h, &image.pixels)?;
                (w, h)
            };
            println!(
                "wrote {} ({}x{}, az={}°, alt={}°, z={})",
                output.display(),
                out.0,
                out.1,
                azimuth,
                altitude,
                z_factor
            );
        }
        DemCommand::Roundtrip { path } => {
            let dem = read_dem(&path)?;
            let encoded = dem.write()?;
            let reparsed = DemFile::parse(&encoded)?;
            if dem.heights != reparsed.heights {
                bail!("heightfield mismatch after round-trip");
            }
            if dem.palette != reparsed.palette {
                bail!("palette mismatch after round-trip");
            }
            println!(
                "OK: {} samples, {} encoded bytes, dimensions {}x{}",
                reparsed.heights.len(),
                encoded.len(),
                reparsed.width(),
                reparsed.height()
            );
        }
    }
    Ok(())
}

fn run_terrain(command: TerrainCommand) -> Result<()> {
    match command {
        TerrainCommand::Generate(args) => generate(args)?,
        TerrainCommand::Import(args) => import(args)?,
        TerrainCommand::Export {
            project,
            output,
            nodata,
            json,
        } => export_terrain_command(&project, &output, nodata, json)?,
        TerrainCommand::Info { project, json } => print_project_info(&project, json)?,
        TerrainCommand::Rock {
            project,
            preset,
            out,
        } => apply_rock(&project, &preset, out.as_deref())?,
        TerrainCommand::Resample {
            project,
            width,
            height,
            filter,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let height = height.unwrap_or(width);
            let hf = vf_terrain::resample(&loaded.heightfield, width, height, filter.into())?;
            let mut p = loaded.project;
            p.sync_terrain(&hf);
            p.camera = vf_scene::auto_frame(
                &hf,
                f64::from(p.render.width.max(1)) / f64::from(p.render.height.max(1)),
            );
            let out = out.unwrap_or(project);
            vf_io::save_project(&p, &hf, &out, loaded.preview.as_ref())?;
            println!(
                "resampled to {}x{} with {:?}, wrote {}",
                hf.width(),
                hf.height(),
                filter,
                out.display()
            );
        }
        TerrainCommand::Sculpt(args) => run_sculpt(args)?,
        TerrainCommand::World(args) => run_world(args)?,
        TerrainCommand::Modifier { command } => run_modifier(command)?,
        TerrainCommand::Smooth {
            project,
            radius,
            iterations,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let hf = vf_terrain::smooth(&loaded.heightfield, radius, iterations);
            let mut p = loaded.project;
            p.sync_terrain(&hf);
            let out = out.unwrap_or(project);
            vf_io::save_project(&p, &hf, &out, loaded.preview.as_ref())?;
            println!(
                "smoothed (radius {radius}, {iterations} iterations), wrote {}",
                out.display()
            );
        }
    }
    Ok(())
}

fn run_world(args: WorldArgs) -> Result<()> {
    let loaded = vf_io::load_project(&args.project)?;
    let mut project = loaded.project;
    let mode = vf_core::WorldMode::parse(&args.mode)
        .ok_or_else(|| anyhow::anyhow!("unknown world mode '{}'", args.mode))?;
    project.world.mode = mode;
    project.world.tiles_x = args.tiles_x;
    project.world.tiles_y = args.tiles_y;
    project.world.blend_cells = args.blend;
    project.world.variation.flip_and_rotate = !args.no_variation;
    project.world.macro_variation_m = args.macro_variation;
    project.world.macro_scale_m = args.macro_scale;
    project
        .world
        .validate()
        .map_err(|e| anyhow::anyhow!("world: {e}"))?;
    let source = &loaded.heightfield;
    let (ow, oh) = project.world.output_size(source.width(), source.height());
    let target = args.out.clone().unwrap_or(args.project.clone());
    if args.apply {
        let composed = vf_terrain::world::compose(source, &project.world, project.seed)
            .map_err(|e| anyhow::anyhow!("world compose: {e}"))?;
        project.sync_terrain(&composed);
        project.camera = vf_scene::auto_frame(
            &composed,
            f64::from(project.render.width.max(1)) / f64::from(project.render.height.max(1)),
        );
        vf_io::save_project(&project, &composed, &target, loaded.preview.as_ref())?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "project": target,
                    "mode": mode.name(),
                    "tiles": [args.tiles_x, args.tiles_y],
                    "size": [composed.width(), composed.height()],
                    "blend_cells": args.blend,
                }))?
            );
        } else {
            println!(
                "composed {}x{} world ({} tiles) -> {}",
                composed.width(),
                composed.height(),
                args.tiles_x * args.tiles_y,
                target.display()
            );
        }
    } else {
        vf_io::save_project(&project, source, &target, loaded.preview.as_ref())?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "project": target,
                    "mode": mode.name(),
                    "tiles": [args.tiles_x, args.tiles_y],
                    "output_size": [ow, oh],
                }))?
            );
        } else {
            println!(
                "world settings saved to {} ({}×{}, output {}×{} with --apply)",
                target.display(),
                args.tiles_x,
                args.tiles_y,
                ow,
                oh
            );
        }
    }
    Ok(())
}

fn run_sculpt(args: SculptArgs) -> Result<()> {
    use vf_terrain::brush::{Brush, BrushMode, BrushShape, Falloff, Stroke, Symmetry};

    let loaded = vf_io::load_project(&args.project)?;
    let mut project = loaded.project;
    let mut hf = loaded.heightfield;
    let mode = BrushMode::parse(&args.mode)
        .ok_or_else(|| anyhow::anyhow!("unknown brush mode '{}'", args.mode))?;
    let shape = BrushShape::parse(&args.shape)
        .ok_or_else(|| anyhow::anyhow!("unknown brush shape '{}'", args.shape))?;
    let falloff = Falloff::parse(&args.falloff)
        .ok_or_else(|| anyhow::anyhow!("unknown brush falloff '{}'", args.falloff))?;
    let symmetry = match args.symmetry.to_ascii_lowercase().as_str() {
        "none" => Symmetry::None,
        "x" => Symmetry::X,
        "y" => Symmetry::Y,
        "xy" => Symmetry::Xy,
        "radial" => Symmetry::Radial(args.radial),
        other => bail!("unknown symmetry '{other}'"),
    };
    let brush = Brush {
        shape,
        mode,
        falloff,
        radius_m: args.radius,
        strength: args.strength,
        roughness: args.roughness,
        hardness: args.hardness,
        aa_samples: args.aa,
        aspect: args.aspect,
        angle_deg: args.angle,
        step_m: args.step,
        dip_deg: args.dip,
        dip_azimuth_deg: args.dip_azimuth,
        bench_ratio: args.bench_ratio,
        bench_variation: args.bench_variation,
        riser_sharpness: args.riser_sharpness,
        column_spacing_m: args.column_spacing,
        column_depth_m: args.column_depth,
        column_jitter: args.column_jitter,
        invert: args.invert,
        symmetry,
        ..Brush::default()
    };
    brush
        .validate()
        .map_err(|e| anyhow::anyhow!("brush: {e}"))?;
    let mut stroke = Stroke::begin(&hf, project.seed, format!("sculpt {}", mode.name()));
    stroke.stamp_to(&mut hf, &brush, args.x, args.y);
    if let (Some(x2), Some(y2)) = (args.x2, args.y2) {
        stroke.stamp_to(&mut hf, &brush, x2, y2);
    }
    let undo = stroke.finish(&hf);
    let region = undo.as_ref().map(|u| u.region);
    project.sync_terrain(&hf);
    let target = args.out.unwrap_or(args.project);
    vf_io::save_project(&project, &hf, &target, None)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "project": target,
                "mode": mode.name(),
                "shape": shape.name(),
                "falloff": falloff.name(),
                "region": region.map(|r| json!({"x0": r.x0, "y0": r.y0, "x1": r.x1, "y1": r.y1})),
                "undo_bytes": undo.as_ref().map(|u| u.bytes()).unwrap_or(0),
                "min": hf.stats().min,
                "max": hf.stats().max,
            }))?
        );
    } else {
        println!(
            "sculpted {} at ({}, {}) -> {} ({}x{} samples, relief {:.1} m)",
            mode.name(),
            args.x,
            args.y,
            target.display(),
            hf.width(),
            hf.height(),
            hf.stats().relief()
        );
    }
    Ok(())
}

fn run_modifier(command: ModifierCommand) -> Result<()> {
    match command {
        ModifierCommand::List { project, json } => {
            let loaded = vf_io::load_project(&project)?;
            let stack = &loaded.project.modifiers;
            if json {
                let items: Vec<_> = stack
                    .items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let params: Vec<_> = item
                            .specs()
                            .iter()
                            .map(|spec| {
                                json!({
                                    "key": spec.key,
                                    "label": spec.label,
                                    "unit": spec.unit,
                                    "value": item.value(spec.key),
                                    "min": spec.min,
                                    "max": spec.max,
                                    "default": spec.default,
                                    "integer": spec.integer,
                                    "logarithmic": spec.logarithmic,
                                    "choices": spec.choices,
                                })
                            })
                            .collect();
                        json!({
                            "index": index,
                            "kind": item.kind.name(),
                            "label": item.kind.label(),
                            "enabled": item.enabled,
                            "summary": item.summary(),
                            "params": params,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "project": project,
                        "count": stack.items.len(),
                        "items": items,
                    }))?
                );
            } else if stack.items.is_empty() {
                println!("modifier stack is empty");
            } else {
                println!("modifier stack ({} entries):", stack.items.len());
                for (index, item) in stack.items.iter().enumerate() {
                    println!(
                        "{index:2}. {} {:<10} {}",
                        if item.enabled { "on " } else { "off" },
                        item.kind.name(),
                        item.summary()
                    );
                    let params: Vec<String> = item
                        .specs()
                        .iter()
                        .map(|spec| {
                            if !spec.choices.is_empty() {
                                let label = spec
                                    .choices
                                    .iter()
                                    .find(|(value, _)| (*value - item.value(spec.key)).abs() < 0.5)
                                    .map_or("?", |(_, label)| *label);
                                format!("{}={label}", spec.key)
                            } else {
                                format!("{}={}", spec.key, item.value(spec.key))
                            }
                        })
                        .collect();
                    println!("    {}", params.join("  "));
                }
                println!("\nuse `terrain modifier set <project> <index> --param key=value`");
            }
        }
        ModifierCommand::Add {
            project,
            kind,
            params,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            let kind = ModifierKind::parse(&kind).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown modifier '{kind}' (expected one of: {})",
                    ModifierKind::ALL
                        .iter()
                        .map(|kind| kind.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let index = p.modifiers.push(kind);
            apply_modifier_params(&mut p.modifiers.items[index], &params)?;
            p.modifiers.normalize();
            p.modifiers.validate().map_err(anyhow::Error::msg)?;
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            println!(
                "added modifier {index}: {} ({}), wrote {}",
                kind.name(),
                p.modifiers.items[index].summary(),
                target.display()
            );
            println!(
                "the stack is applied at render time; use `terrain modifier bake` to freeze it"
            );
        }
        ModifierCommand::Set {
            project,
            index,
            params,
            enable,
            disable,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            if index >= p.modifiers.items.len() {
                bail!(
                    "modifier index {index} out of range (stack has {} entries)",
                    p.modifiers.items.len()
                );
            }
            if enable {
                p.modifiers.items[index].enabled = true;
            }
            if disable {
                p.modifiers.items[index].enabled = false;
            }
            apply_modifier_params(&mut p.modifiers.items[index], &params)?;
            p.modifiers.normalize();
            p.modifiers.validate().map_err(anyhow::Error::msg)?;
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            println!(
                "modified {index}: {}, wrote {}",
                p.modifiers.items[index].summary(),
                target.display()
            );
        }
        ModifierCommand::Remove {
            project,
            index,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            let removed = p
                .modifiers
                .remove(index)
                .ok_or_else(|| anyhow::anyhow!("modifier index {index} out of range"))?;
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            println!(
                "removed {index}: {}, wrote {}",
                removed.summary(),
                target.display()
            );
        }
        ModifierCommand::Move {
            project,
            index,
            up,
            down,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            let moved = if up {
                p.modifiers.move_up(index)
            } else if down {
                p.modifiers.move_down(index)
            } else {
                bail!("specify --up or --down");
            };
            if !moved {
                bail!("cannot move modifier {index} in that direction");
            }
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            println!("moved modifier {index}, wrote {}", target.display());
        }
        ModifierCommand::Clear { project, out } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            let count = p.modifiers.items.len();
            p.modifiers.clear();
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            println!("cleared {count} modifiers, wrote {}", target.display());
        }
        ModifierCommand::Bake { project, out } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            let before = loaded.heightfield.stats();
            let shaped = vf_modifiers::apply(&loaded.heightfield, &p.modifiers, p.seed)?;
            p.modifiers.clear();
            p.sync_terrain(&shaped);
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &shaped, &target, loaded.preview.as_ref())?;
            let after = shaped.stats();
            println!(
                "baked modifiers: relief {:.1} -> {:.1} m, wrote {}",
                before.relief(),
                after.relief(),
                target.display()
            );
        }
    }
    Ok(())
}

fn apply_modifier_params(modifier: &mut vf_core::Modifier, params: &[String]) -> Result<()> {
    for raw in params {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("parameter '{raw}' must be key=value"))?;
        let key = key.trim();
        let raw = value.trim();
        // Enumerated parameters accept either their numeric index or their
        // label (`operation=add`, `repeat=oui`).
        let value = if let Ok(value) = raw.parse::<f32>() {
            value
        } else {
            modifier
                .specs()
                .iter()
                .find(|spec| spec.key == key)
                .and_then(|spec| {
                    spec.choices
                        .iter()
                        .find(|(_, label)| label.eq_ignore_ascii_case(raw))
                        .map(|(value, _)| *value)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("parameter '{key}' expects a number or one of its labels")
                })?
        };
        if !modifier.set(key, value) {
            bail!(
                "{} has no parameter '{key}' (expected one of: {})",
                modifier.kind.name(),
                modifier
                    .specs()
                    .iter()
                    .map(|spec| spec.key)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}

fn apply_rock(project_path: &Path, preset: &str, out: Option<&Path>) -> Result<()> {
    let loaded = vf_io::load_project(project_path)?;
    let settings = GeologySettings::preset(preset)
        .ok_or_else(|| anyhow::anyhow!("unknown rock preset '{preset}'"))?;
    let seed = loaded.project.seed;
    // Applied to the current surface: for a DEM this is exactly what we want,
    // and no fractal regeneration is involved.
    let shaped = vf_geology::apply(&loaded.heightfield, &settings, seed)?;
    let shaped = vf_geology::micro_detail(&shaped, &settings, seed)?;
    let mut project = loaded.project;
    project.geology = settings;
    if let Some(detail) = vf_core::DetailSettings::preset(preset) {
        project.detail = detail;
    }
    project.sync_terrain(&shaped);
    project.mark_modified();
    let target = out.unwrap_or(project_path);
    vf_io::save_project(&project, &shaped, target, None)?;
    let stats = shaped.stats();
    println!(
        "applied rock '{preset}' to {} -> {} (relief {:.1} m)",
        project_path.display(),
        target.display(),
        stats.relief()
    );
    Ok(())
}

fn generate(args: GenerateArgs) -> Result<()> {
    let start = Instant::now();
    if args.size > 16_384 {
        bail!("terrain size is capped at 16384 samples per axis");
    }
    if args.size > 8192 {
        let bytes = u64::from(args.size) * u64::from(args.size) * 4;
        println!(
            "warning: {}² terrain needs at least {:.1} GiB for the heightfield;              use the tiled world mode if memory is tight",
            args.size,
            bytes as f64 / 1_073_741_824.0
        );
    }
    let seed = args.seed.map_or_else(Seed::from_time, Seed::new);
    let name = args.name.clone().unwrap_or_else(|| match args.mode {
        ModeArg::Classic => format!("Classic {seed}"),
        ModeArg::Modern => format!("{} {seed}", args.preset),
    });
    let mut project = Project::new(name.clone(), seed, args.size);
    let erosion = if args.erode {
        let mut params = ErosionParams::scaled_for(args.size);
        if let Some(droplets) = args.droplets {
            params.droplets = droplets;
        }
        Some(params)
    } else {
        None
    };

    project.source = match args.mode {
        ModeArg::Classic => {
            let mut params = ClassicParams::default().with_size_seed(args.size, seed.0);
            if let Some(dimension) = args.dimension {
                params.fractal_dimension = dimension;
            }
            if let Some(divisor) = args.divisor {
                params.divisor = divisor;
            }
            params.island = args.island;
            if let Some(scale) = args.vertical_scale {
                params.vertical_scale = scale;
            }
            TerrainSource::Classic { params }
        }
        ModeArg::Modern => {
            let mut params = vf_core::terrain_params::modern_preset(&args.preset)
                .ok_or_else(|| anyhow::anyhow!("unknown modern preset '{}'", args.preset))?
                .with_size_seed(args.size, seed.0);
            if let Some(scale) = args.vertical_scale {
                params.vertical_scale = scale;
            }
            if let Some(ridge) = args.ridge {
                params.ridge_strength = ridge;
            }
            if let Some(warp) = args.warp {
                params.warp_strength = warp;
            }
            if let Some(continent) = args.continent {
                params.continent = continent;
            }
            if let Some(falloff) = args.border_falloff {
                params.border_falloff = falloff.clamp(0.0, 1.0);
            }
            TerrainSource::Modern { params }
        }
    };

    project.erosion_enabled = erosion.is_some();
    if let Some(params) = &erosion {
        project.erosion = params.clone();
    }

    // The modern preset carries a matching palette; an explicit --palette wins.
    if let TerrainSource::Modern { params } = &project.source
        && let Some(palette) = Palette::preset(&params.preset)
    {
        project.palette = palette;
    }
    if let Some(name) = &args.palette {
        project.palette =
            Palette::preset(name).ok_or_else(|| anyhow::anyhow!("unknown palette '{name}'"))?;
    }
    project.geology = match &args.rock {
        Some(name) => GeologySettings::preset(name)
            .ok_or_else(|| anyhow::anyhow!("unknown rock preset '{name}'"))?,
        None => GeologySettings::default(),
    };
    // The continuous detail profile follows the rock family unless the user
    // explicitly chooses one. It is evaluated at render time, so it does not
    // change the stored DEM or the erosion.
    if let Some(profile) = args.detail_profile.as_ref().or(args.rock.as_ref()) {
        project.detail = vf_core::DetailSettings::preset(profile).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown detail profile '{profile}' (known: {})",
                vf_core::DETAIL_PRESETS.join(", ")
            )
        })?;
    }
    if args.no_detail {
        project.detail.enabled = false;
    }
    project
        .detail
        .validate()
        .map_err(|e| anyhow::anyhow!("detail: {e}"))?;
    project
        .materials
        .validate()
        .map_err(|e| anyhow::anyhow!("material: {e}"))?;
    project.erosion_enabled = erosion.is_some();
    if let Some(params) = &erosion {
        project.erosion = params.clone();
    }
    if args.rock.is_some() {
        println!(
            "applying rock preset '{}'",
            args.rock.as_deref().unwrap_or("")
        );
        if project.geology.auto_palette
            && let Some(palette) = Palette::preset(project.geology.suggested_palette())
        {
            project.palette = palette;
        }
    }
    let hf = vf_geology::build(&project.source, erosion.as_ref(), &project.geology, seed)?;
    project.sync_terrain(&hf);
    project.camera = vf_scene::auto_frame(
        &hf,
        f64::from(project.render.width) / f64::from(project.render.height.max(1)),
    );
    project.mark_modified();

    let out = args
        .out
        .unwrap_or_else(|| PathBuf::from(format!("{}.vfproj", slug(&name))));
    vf_io::save_project(&project, &hf, &out, None)?;
    let stats = hf.stats();
    println!(
        "generated {} {}x{} seed={} in {} ms",
        project.source.kind(),
        hf.width(),
        hf.height(),
        seed,
        start.elapsed().as_millis()
    );
    println!(
        "elevation: {:.1}..{:.1} m (mean {:.1}), relief {:.1} m",
        stats.min,
        stats.max,
        stats.mean,
        stats.relief()
    );
    println!(
        "wrote {} ({} bytes)",
        out.display(),
        std::fs::metadata(&out)?.len()
    );

    if let Some(render_path) = args.render {
        let options = RenderOptions {
            width: args.render_width,
            height: args.render_height,
            ssaa: 1,
            shadows: true,
            shadow_resolution: 2048,
            ao: false,
            ..RenderOptions::default()
        };
        render_project(&project, &hf, &options, &render_path, false)?;
    }
    Ok(())
}

fn import(args: ImportArgs) -> Result<()> {
    let start = Instant::now();
    let options = vf_io::TerrainImportOptions {
        heightmap: vf_io::HeightmapOptions {
            min_height: args.min_height,
            max_height: args.max_height,
            spacing_m: args.spacing.map(f64::from).unwrap_or(0.0),
            origin: [0.0, 0.0],
            flip_y: !args.no_flip_y,
            invert_height: args.invert_height,
        },
        dem_spacing_m: args.spacing.unwrap_or(30.0),
    };
    let (hf, source, description) = if args.ascii {
        let mut hf = vf_io::ascii::read_ascii_grid(&args.input)?;
        if let Some(spacing) = args.spacing {
            hf.set_spacing(f64::from(spacing), f64::from(spacing))?;
        }
        let title = args.input.file_stem().map_or_else(
            || "ASCII grid".to_string(),
            |s| s.to_string_lossy().into_owned(),
        );
        let spacing_m = hf.spacing().0 as f32;
        (
            hf,
            TerrainSource::Dem {
                source: DemSource {
                    path: args.input.display().to_string(),
                    title: title.clone(),
                    comment: "Imported ESRI ASCII Grid".into(),
                    spacing_m,
                },
            },
            "ESRI ASCII Grid".to_string(),
        )
    } else {
        let imported = vf_io::import_terrain(&args.input, options)?;
        (imported.heightfield, imported.source, imported.description)
    };
    let title = match &source {
        TerrainSource::Dem { source } => source.title.clone(),
        _ => "Imported terrain".into(),
    };
    let project_name = args.name.unwrap_or_else(|| title.clone());
    let seed = Seed::new(fnv1a(&title));
    let mut project = Project::new(project_name, seed, hf.width() as u32);
    project.source = source;
    project.legacy = Some(LegacyMeta {
        dem_title: title,
        dem_comment: description.clone(),
        ..LegacyMeta::default()
    });
    // Inland DEMs rarely need a global sea plane; users can enable it later.
    project.water.enabled = false;
    project.sync_terrain(&hf);
    project.camera = vf_scene::auto_frame(
        &hf,
        f64::from(project.render.width) / f64::from(project.render.height.max(1)),
    );
    let out = args
        .out
        .unwrap_or_else(|| args.input.with_extension("vfproj"));
    vf_io::save_project(&project, &hf, &out, None)?;
    let stats = hf.stats();
    println!(
        "imported {}x{} ({}) into {} in {} ms",
        hf.width(),
        hf.height(),
        description,
        out.display(),
        start.elapsed().as_millis()
    );
    println!(
        "elevation: {:.1}..{:.1} m (mean {:.1}), spacing {:.1} m",
        stats.min,
        stats.max,
        stats.mean,
        hf.spacing().0
    );
    if let Some(render_path) = args.render {
        let options = RenderOptions {
            width: args.render_width,
            height: args.render_height,
            ssaa: 1,
            shadows: true,
            shadow_resolution: 2048,
            ao: false,
            ..RenderOptions::default()
        };
        render_project(&project, &hf, &options, &render_path, false)?;
    }
    Ok(())
}
fn run_scene(command: SceneCommand) -> Result<()> {
    match command {
        SceneCommand::Preset {
            project,
            name,
            out,
            no_reframe,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            p.apply_scene_preset(&name).map_err(anyhow::Error::msg)?;
            if !no_reframe {
                let aspect = f64::from(p.render.width.max(1)) / f64::from(p.render.height.max(1));
                p.camera = vf_scene::auto_frame(&loaded.heightfield, aspect);
            }
            let out = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &out, loaded.preview.as_ref())?;
            println!("applied scene preset '{name}', wrote {}", out.display());
        }
    }
    Ok(())
}

fn load_flight(project: &Project, preset: Option<&str>) -> Result<(FlightSettings, Seed)> {
    let settings = match preset {
        Some(name) => FlightSettings::preset(name)
            .ok_or_else(|| anyhow::anyhow!("unknown flight preset '{name}'"))?,
        None if project.flight.enabled => project.flight.clone(),
        None => FlightSettings::preset("cinelifter").expect("built-in preset"),
    };
    settings
        .validate()
        .map_err(|e| anyhow::anyhow!("flight: {e}"))?;
    let seed = Seed::new(settings.seed);
    Ok((settings, seed))
}

fn render_options(
    project: &Project,
    width: Option<u32>,
    height: Option<u32>,
    quality: Option<QualityArg>,
    exposure: Option<f32>,
) -> RenderOptions {
    let mut settings = project.render.clone();
    if let Some(quality) = quality {
        settings.apply_quality(quality.into());
    }
    if let Some(width) = width {
        settings.width = width;
    }
    if let Some(height) = height {
        settings.height = height;
    }
    if let Some(exposure) = exposure {
        settings.exposure = exposure;
    }
    RenderOptions::from(&settings)
}

/// Encode the PNG sequence with ffmpeg when it is available. No bundled
/// encoder: the sequence is the deliverable, the MP4 is a convenience.
fn encode_video(frames: &Path, fps: u32, output: &Path) -> Result<()> {
    let available = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        println!(
            "ffmpeg not found; frames kept in {}. Encode with:\n  ffmpeg -framerate {} -i {}/frame_%05d.png -c:v libx264 -crf 16 -pix_fmt yuv420p {}",
            frames.display(),
            fps,
            frames.display(),
            output.display()
        );
        return Ok(());
    }
    let status = std::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-framerate")
        .arg(fps.to_string())
        .arg("-i")
        .arg(frames.join("frame_%05d.png"))
        .args(["-c:v", "libx264", "-crf", "16", "-pix_fmt", "yuv420p"])
        .arg("-movflags")
        .arg("+faststart")
        .arg(output)
        .status()?;
    if !status.success() {
        anyhow::bail!("ffmpeg failed with {status}");
    }
    println!("wrote {}", output.display());
    Ok(())
}

/// The terrain the flight should follow: the base heightfield with the live
/// modifier stack applied. Rendering still receives the base field, because
/// `vf-render` applies the stack itself exactly once.
fn live_heightfield(project: &Project, hf: &HeightField) -> Result<HeightField> {
    if project.modifiers.active().next().is_some() {
        vf_modifiers::apply(hf, &project.modifiers, project.seed)
            .map_err(|e| anyhow::anyhow!("modifier: {e}"))
    } else {
        Ok(hf.clone())
    }
}

fn run_anim(command: AnimCommand) -> Result<()> {
    match command {
        AnimCommand::Still(args) => {
            let loaded = vf_io::load_project(&args.project)?;
            let live = live_heightfield(&loaded.project, &loaded.heightfield)?;
            if loaded.project.rig.enabled {
                let sample = vf_flight::rig::sample_rig(
                    &loaded.project.rig,
                    &live,
                    loaded.project.seed,
                    args.time as f32,
                )
                .map_err(|e| anyhow::anyhow!("rig: {e}"))?;
                let camera = sample.camera.clone();
                let mut project = loaded.project;
                let fov = camera.fov_y_deg;
                project.camera = camera;
                let options = render_options(
                    &project,
                    args.width,
                    args.height,
                    args.quality,
                    args.exposure,
                );
                let output = render_with_options(&project, &loaded.heightfield, &options)?;
                output.image.save_png(&args.output)?;
                println!(
                    "rig still t={:.2}s preset={} -> {} ({}x{}, fov {:.0}°)",
                    args.time,
                    project.rig.body.preset.name(),
                    args.output.display(),
                    output.stats.width,
                    output.stats.height,
                    fov
                );
                println!(
                    "  agl {:.1} m · bank {:.1}° · roll {:.1}° · speed {:.1} m/s",
                    sample.altitude_agl_m, sample.bank_deg, sample.roll_deg, sample.speed_mps
                );
                return Ok(());
            }
            let (flight, seed) = load_flight(&loaded.project, args.preset.as_deref())?;
            let sample = vf_flight::sample_at(
                &flight,
                &live,
                seed,
                loaded.project.camera.fov_y_deg,
                args.time,
            )?;
            let camera = sample.camera.clone();
            let forward = (camera.target - camera.position).normalize_or_zero();
            let pitch_deg = forward.z.asin().to_degrees();
            let mut project = loaded.project;
            project.camera = camera;
            let options = render_options(
                &project,
                args.width,
                args.height,
                args.quality,
                args.exposure,
            );
            let output = render_with_options(&project, &loaded.heightfield, &options)?;
            output.image.save_png(&args.output)?;
            println!(
                "flight still t={:.2}s preset={} -> {} ({}x{})",
                args.time,
                flight.preset,
                args.output.display(),
                output.stats.width,
                output.stats.height
            );
            println!(
                "  agl {:.1} m · bank {:.1}° · pitch {:.1}° · speed {:.1} m/s",
                sample.altitude_agl_m, sample.bank_deg, pitch_deg, sample.speed_mps
            );
            Ok(())
        }
        AnimCommand::Render(args) => {
            let loaded = vf_io::load_project(&args.project)?;
            if loaded.project.rig.enabled {
                let mut rig = loaded.project.rig.clone();
                if let Some(seconds) = args.seconds {
                    rig.duration_s = seconds;
                }
                if let Some(fps) = args.fps {
                    rig.fps = fps;
                }
                rig.validate().map_err(|e| anyhow::anyhow!("rig: {e}"))?;
                let live = live_heightfield(&loaded.project, &loaded.heightfield)?;
                let mut simulation =
                    vf_flight::rig::RigSimulation::new(&rig, &live, loaded.project.seed)
                        .map_err(|e| anyhow::anyhow!("rig: {e}"))?;
                std::fs::create_dir_all(&args.out)?;
                let mut project = loaded.project;
                let stride = args.stride.max(1);
                let frames = rig.frame_count();
                let started = Instant::now();
                let mut written = 0usize;
                for index in 0..frames {
                    let time = index as f32 / rig.fps.max(1) as f32;
                    let sample = simulation.sample_at(time);
                    if index % stride != 0 {
                        continue;
                    }
                    project.camera = sample.camera;
                    let options =
                        render_options(&project, args.width, args.height, args.quality, None);
                    let output = render_with_options(&project, &loaded.heightfield, &options)?;
                    let path = args.out.join(format!("frame_{index:05}.png"));
                    output.image.save_png(&path)?;
                    written += 1;
                    if written.is_multiple_of(8) || index + 1 == frames {
                        println!(
                            "rig frame {}/{} ({} written, {:.1}s elapsed)",
                            index + 1,
                            frames,
                            written,
                            started.elapsed().as_secs_f64()
                        );
                    }
                }
                println!(
                    "wrote {} rig frames to {} in {:.1}s (preset {}, {:.1}s @ {} fps)",
                    written,
                    args.out.display(),
                    started.elapsed().as_secs_f64(),
                    rig.body.preset.name(),
                    rig.duration_s,
                    rig.fps
                );
                if let Some(video) = &args.video {
                    encode_video(&args.out, rig.fps, video)?;
                }
                return Ok(());
            }
            let (mut flight, seed) = load_flight(&loaded.project, args.preset.as_deref())?;
            if let Some(seconds) = args.seconds {
                flight.duration_s = seconds;
            }
            if let Some(fps) = args.fps {
                flight.fps = fps;
            }
            flight
                .validate()
                .map_err(|e| anyhow::anyhow!("flight: {e}"))?;
            let live = live_heightfield(&loaded.project, &loaded.heightfield)?;
            let samples =
                vf_flight::simulate(&flight, &live, seed, loaded.project.camera.fov_y_deg)?;
            std::fs::create_dir_all(&args.out)?;
            let mut project = loaded.project;
            let stride = args.stride.max(1);
            let total = samples.len();
            let started = Instant::now();
            let mut written = 0usize;
            let shutter = args.shutter.clamp(0.0, 360.0);
            let blur_samples = args.blur_samples.clamp(1, 32);
            for (index, sample) in samples.iter().enumerate() {
                if index % stride != 0 {
                    continue;
                }
                let options = render_options(&project, args.width, args.height, args.quality, None);
                let image = if shutter > 0.0 {
                    // Accumulate sub-frames inside the exposure window: real
                    // motion blur, not a directional post filter.
                    let exposure = f64::from(shutter) / 360.0 / f64::from(flight.fps);
                    let mut accumulator: Vec<u32> = Vec::new();
                    for k in 0..blur_samples {
                        let offset = if blur_samples == 1 {
                            0.0
                        } else {
                            (f64::from(k) / f64::from(blur_samples - 1) - 0.5) * exposure
                        };
                        let t = sample.time_s + offset;
                        let sub = vf_flight::sample_at(
                            &flight,
                            &live,
                            seed,
                            project.camera.fov_y_deg,
                            t.max(0.0),
                        )?;
                        let mut sub_project = project.clone();
                        sub_project.camera = sub.camera;
                        let output =
                            render_with_options(&sub_project, &loaded.heightfield, &options)?;
                        if accumulator.is_empty() {
                            accumulator =
                                output.image.pixels.iter().map(|&v| u32::from(v)).collect();
                        } else {
                            for (slot, &v) in accumulator.iter_mut().zip(output.image.pixels.iter())
                            {
                                *slot += u32::from(v);
                            }
                        }
                    }
                    let n = blur_samples;
                    let pixels = accumulator.iter().map(|&v| (v / n) as u8).collect();
                    vf_io::RgbaImage::new(options.width, options.height, pixels)?
                } else {
                    project.camera = sample.camera.clone();
                    render_with_options(&project, &loaded.heightfield, &options)?.image
                };
                let path = args.out.join(format!("frame_{index:05}.png"));
                image.save_png(&path)?;
                written += 1;
                if written.is_multiple_of(8) || index + 1 == total {
                    println!(
                        "frame {}/{} ({} written, {:.1}s elapsed)",
                        index + 1,
                        total,
                        written,
                        started.elapsed().as_secs_f64()
                    );
                }
            }
            println!(
                "wrote {} frames to {} in {:.1}s (preset {}, {:.1}s @ {} fps)",
                written,
                args.out.display(),
                started.elapsed().as_secs_f64(),
                flight.preset,
                flight.duration_s,
                flight.fps
            );
            if let Some(video) = &args.video {
                encode_video(&args.out, flight.fps, video)?;
            }
            Ok(())
        }
    }
}

fn run_render(args: RenderArgs) -> Result<()> {
    let loaded = vf_io::load_project(&args.project)?;
    let mut project = loaded.project;
    let hf = loaded.heightfield;

    if args.auto_frame {
        let width = args.width.unwrap_or(project.render.width).max(1);
        let height = args.height.unwrap_or(project.render.height).max(1);
        project.camera = vf_scene::auto_frame(&hf, f64::from(width) / f64::from(height));
    }
    if let Some(value) = &args.camera {
        project.camera.position = parse_vec3(value, "camera")?;
    }
    if let Some(value) = &args.target {
        project.camera.target = parse_vec3(value, "target")?;
    }
    if let Some(fov) = args.fov {
        project.camera.fov_y_deg = fov;
    }
    if let Some(value) = &args.sun {
        let (azimuth, declination) = parse_pair(value, "sun")?;
        project.sun.azimuth_deg_from_south = azimuth;
        project.sun.declination_deg = declination;
    }
    if let Some(name) = &args.detail_profile {
        if !project.detail.apply_preset(name) {
            anyhow::bail!(
                "unknown detail profile '{name}' (known: {})",
                vf_core::DETAIL_PRESETS.join(", ")
            );
        }
        project.detail.enabled = true;
    }
    if args.detail {
        project.detail.enabled = true;
        if project.detail.profile == "generic" && project.detail.amplitude_m <= 0.0 {
            project.detail = vf_core::DetailSettings::preset("generic").unwrap();
        }
    }
    if args.no_detail {
        project.detail.enabled = false;
    }
    if let Some(strength) = args.detail_strength {
        project.detail.strength = strength;
        project.detail.enabled = true;
    }
    if let Some(scale) = args.detail_scale {
        project.detail.scale_m = scale;
        project.detail.enabled = true;
    }
    if let Some(amplitude) = args.detail_amplitude {
        project.detail.amplitude_m = amplitude;
        project.detail.enabled = true;
    }
    if let Some(value) = &args.detail_focus {
        let (x, y) = parse_pair(value, "detail-focus")?;
        project.detail.focus_center = [f64::from(x), f64::from(y)];
        project.detail.focus_enabled = true;
        project.detail.enabled = true;
    }
    if let Some(size) = args.detail_focus_size {
        project.detail.focus_size_m = size;
        project.detail.focus_enabled = true;
        project.detail.enabled = true;
    }
    if let Some(spacing) = args.detail_focus_spacing {
        project.detail.focus_spacing_m = spacing;
        project.detail.focus_enabled = true;
        project.detail.enabled = true;
    }
    if let Some(strength) = args.detail_normal {
        project.materials.bump_strength = strength;
    }
    if let Some(name) = &args.rock_material {
        let key = name.trim().to_ascii_lowercase();
        if !vf_core::ROCK_PRESETS.contains(&key.as_str()) {
            anyhow::bail!(
                "unknown rock material '{name}' (known: {})",
                vf_core::ROCK_PRESETS.join(", ")
            );
        }
        project.materials.rock_preset = key;
    }
    if let Some(value) = args.bump {
        project.materials.bump_strength = value;
    }
    if let Some(value) = args.bump_scale {
        project.materials.bump_scale_m = value;
    }
    if let Some(value) = args.displacement {
        project.materials.displacement_m = value;
    }
    if let Some(value) = &args.texture {
        project.materials.texture_path = value.clone();
    }
    if let Some(value) = args.texture_scale {
        project.materials.texture_scale_m = value;
    }
    if let Some(value) = args.texture_bump {
        project.materials.texture_bump = value;
    }
    if let Some(value) = args.texture_displacement {
        project.materials.texture_displacement_m = value;
    }
    if let Some(value) = args.texture_mix {
        project.materials.texture_mix = value;
    }
    if args.no_shape_adaptive {
        project.detail.adaptive = false;
    }
    if let Some(value) = args.adaptive_tolerance {
        project.detail.adaptive = true;
        project.detail.adaptive_tolerance_m = value;
    }
    if args.quads {
        project.detail.quad_mesh = true;
    }
    if let Some(value) = args.detail_heterogeneity {
        project.detail.heterogeneity = value;
        project.detail.enabled = true;
    }
    if let Some(warp) = args.cliff_warp {
        project.render.cliff_warp = warp;
    }
    if let Some(scale) = args.cliff_warp_scale {
        project.render.cliff_warp_scale_m = scale;
    }
    project
        .detail
        .validate()
        .map_err(|e| anyhow::anyhow!("detail: {e}"))?;
    project
        .camera
        .validate()
        .map_err(|e| anyhow::anyhow!("camera: {e}"))?;
    project
        .sun
        .validate()
        .map_err(|e| anyhow::anyhow!("sun: {e}"))?;

    let mut settings = project.render.clone();
    if let Some(quality) = args.quality {
        settings.apply_quality(quality.into());
    }
    if let Some(mode) = &args.mode {
        let mode = vf_core::RenderMode::parse(mode)
            .ok_or_else(|| anyhow::anyhow!("unknown render mode '{mode}'"))?;
        mode.apply(&mut settings);
    }
    if let Some(width) = args.width {
        settings.width = width;
    }
    if let Some(height) = args.height {
        settings.height = height;
    }
    if let Some(ssaa) = args.ssaa {
        settings.ssaa = ssaa;
    }
    if let Some(subdivision) = args.subdivision {
        settings.subdivision = subdivision;
    }
    if args.no_adaptive {
        settings.adaptive = false;
    }
    if args.shadows {
        settings.shadows = true;
    }
    if args.no_shadows {
        settings.shadows = false;
    }
    if args.ao {
        settings.ao = true;
    }
    if args.no_ao {
        settings.ao = false;
    }
    if args.cliffs {
        settings.cliffs = true;
    }
    if args.no_cliffs {
        settings.cliffs = false;
    }
    if args.clouds {
        settings.clouds = true;
    }
    if args.no_clouds {
        settings.clouds = false;
    }
    if args.water {
        settings.water = true;
    }
    if args.no_water {
        settings.water = false;
    }
    if let Some(exposure) = args.exposure {
        settings.exposure = exposure;
    }

    let mut options = RenderOptions::from(&settings);
    options.detail_geometry = project.detail.enabled;
    if args.preview {
        options.ssaa = 1;
        options.shadows = false;
        options.ao = false;
        if args.width.is_none() {
            options.width = 960;
        }
        if args.height.is_none() {
            options.height = 540;
        }
    }

    let output = render_project(&project, &hf, &options, &args.output, args.json)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "project": args.project,
                "output": args.output,
                "width": output.stats.width,
                "height": output.stats.height,
                "ssaa": output.stats.ssaa,
                "triangles": output.stats.triangles,
                "shadow_resolution": output.stats.shadow_resolution,
                "elapsed_ms": output.stats.elapsed_ms,
            }))?
        );
    }
    Ok(())
}

fn render_project(
    project: &Project,
    hf: &HeightField,
    options: &RenderOptions,
    output: &Path,
    quiet: bool,
) -> Result<vf_render::RenderOutput> {
    let render = render_with_options(project, hf, options)
        .map_err(|e| anyhow::anyhow!("render failed: {e}"))?;
    render
        .save_png(output)
        .with_context(|| format!("cannot write {}", output.display()))?;
    if !quiet {
        println!(
            "rendered {}x{} (ssaa {}, {} triangles) in {} ms -> {}",
            render.stats.width,
            render.stats.height,
            render.stats.ssaa,
            render.stats.triangles,
            render.stats.elapsed_ms,
            output.display()
        );
    }
    Ok(render)
}

fn run_preset(command: PresetCommand) -> Result<()> {
    match command {
        PresetCommand::List { json } => {
            let terrain = vf_core::terrain_params::MODERN_PRESETS;
            let scenes = ["alpine", "sunset", "desert", "volcanic", "lunar", "coastal"];
            let palettes = [
                "alpine", "desert", "volcanic", "coastal", "canyon", "lunar", "island",
            ];
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "terrain": terrain,
                        "rock": vf_core::GEOLOGY_PRESETS,
                        "scene": scenes,
                        "palette": palettes,
                    }))?
                );
            } else {
                println!("terrain: {}", terrain.join(", "));
                println!("rock:    {}", vf_core::GEOLOGY_PRESETS.join(", "));
                println!("flight:  {}", vf_core::FLIGHT_PRESETS.join(", "));
                println!("scene:   {}", scenes.join(", "));
                println!("palette: {}", palettes.join(", "));
            }
        }
    }
    Ok(())
}

fn run_rig(command: RigCommand) -> Result<()> {
    match command {
        RigCommand::Info { project, json } => {
            let loaded = vf_io::load_project(&project)?;
            let rig = &loaded.project.rig;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "enabled": rig.enabled,
                        "name": rig.name,
                        "duration_s": rig.duration_s,
                        "fps": rig.fps,
                        "body": rig.body.preset.name(),
                        "target": format!("{:?}", rig.target.mode),
                        "fov": rig.optics.fov_y_deg,
                        "dof": rig.optics.dof_enabled,
                        "clearance_m": rig.constraints.ground_clearance_m,
                        "keys": rig.eye.keys.iter().map(|k| json!({
                            "time_s": k.time_s,
                            "position": k.position,
                        })).collect::<Vec<_>>(),
                    }))?
                );
            } else {
                println!(
                    "rig:         {} ({})",
                    rig.name,
                    if rig.enabled { "on" } else { "off" }
                );
                println!(
                    "duration:    {:.2}s @ {} fps · body {} · target {:?}",
                    rig.duration_s,
                    rig.fps,
                    rig.body.preset.name(),
                    rig.target.mode
                );
                println!(
                    "optics:      fov {:.0}° · DOF {} · f/{:.1}",
                    rig.optics.fov_y_deg,
                    if rig.optics.dof_enabled { "on" } else { "off" },
                    rig.optics.fstop
                );
                println!(
                    "constraints: sol {:.1} m · paroi {:.1} m · {}",
                    rig.constraints.ground_clearance_m,
                    rig.constraints.wall_distance_m,
                    if rig.constraints.enabled { "on" } else { "off" }
                );
                for (index, key) in rig.eye.keys.iter().enumerate() {
                    println!(
                        "  key {index}: t={:.2}s  ({:.0}, {:.0}, {:.0})",
                        key.time_s, key.position[0], key.position[1], key.position[2]
                    );
                }
            }
            Ok(())
        }
        RigCommand::Preset {
            project,
            preset,
            path,
            duration,
            out,
            json: json_output,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            p.rig = vf_core::CameraRig::preset(&preset)
                .ok_or_else(|| anyhow::anyhow!("unknown rig preset '{preset}'"))?;
            vf_flight::rig::preset_path(&mut p.rig, &loaded.heightfield, &path)
                .map_err(|e| anyhow::anyhow!("rig path: {e}"))?;
            if let Some(duration) = duration {
                p.rig.stretch_duration(duration);
            }
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "project": target,
                        "preset": preset,
                        "path": path,
                        "duration_s": p.rig.duration_s,
                        "keys": p.rig.eye.keys.len(),
                    }))?
                );
            } else {
                println!(
                    "rig '{}' with {} path and {} keys -> {}",
                    preset,
                    path,
                    p.rig.eye.keys.len(),
                    target.display()
                );
            }
            Ok(())
        }
        RigCommand::Key {
            project,
            time,
            x,
            y,
            z,
            out,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let mut p = loaded.project;
            p.rig.eye.push(vf_core::PathKey::new(time, [x, y, z]));
            p.rig.duration_s = p.rig.duration_s.max(time);
            p.rig.validate().map_err(|e| anyhow::anyhow!("rig: {e}"))?;
            p.mark_modified();
            let target = out.unwrap_or(project);
            vf_io::save_project(&p, &loaded.heightfield, &target, loaded.preview.as_ref())?;
            println!(
                "added rig key t={time:.2}s ({x:.0}, {y:.0}, {z:.0}) -> {}",
                target.display()
            );
            Ok(())
        }
        RigCommand::Export {
            project,
            output,
            json,
        } => run_rig_export(&project, &output, json),
        RigCommand::Import {
            project,
            animation,
            out,
            json,
        } => run_rig_import(&project, &animation, out.as_deref(), json),
    }
}

fn run_rig_export(project: &Path, output: &Path, json: bool) -> Result<()> {
    let loaded = vf_io::load_project(project)?;
    let report = vf_export::export_animation(
        output,
        &loaded.project.rig,
        &loaded.heightfield,
        loaded.project.seed,
    )
    .map_err(|error| anyhow::anyhow!("animation export: {error}"))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": report.path,
                "format": report.format,
                "frames": report.frames,
                "duration_s": report.duration_s,
                "has_target": report.has_target,
            }))?
        );
    } else {
        println!(
            "exported {} frames ({:.2}s) to {} ({})",
            report.frames,
            report.duration_s,
            report.path.display(),
            report.format
        );
        if report.path.extension().and_then(|e| e.to_str()) == Some("glb") {
            println!("Blender: File > Import > glTF 2.0, or use bridge/blender/vistaforge_io.py");
        }
    }
    Ok(())
}

fn run_rig_import(project: &Path, animation: &Path, out: Option<&Path>, json: bool) -> Result<()> {
    let loaded = vf_io::load_project(project)?;
    let imported = vf_export::import_animation(animation)
        .map_err(|error| anyhow::anyhow!("animation import: {error}"))?;
    let mut p = loaded.project;
    let keys = imported.rig.eye.keys.len();
    let duration = imported.rig.duration_s;
    let target_mode = format!("{:?}", imported.rig.target.mode);
    let terrain = imported.terrain.clone();
    p.rig = imported.rig;
    p.rig.enabled = true;
    p.mark_modified();
    let target = out.unwrap_or(project);
    vf_io::save_project(&p, &loaded.heightfield, target, loaded.preview.as_ref())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "project": target,
                "source": imported.source,
                "format": imported.format,
                "keys": keys,
                "duration_s": duration,
                "target_mode": target_mode,
                "terrain_reference": terrain,
            }))?
        );
    } else {
        println!(
            "imported {} ({}): {} keys, {:.2}s, target {} -> {}",
            imported.format,
            imported.source,
            keys,
            duration,
            target_mode,
            target.display()
        );
        if let Some(terrain) = terrain {
            println!(
                "source terrain was {}x{} at {:.2} m/cellule",
                terrain.width, terrain.height, terrain.spacing_x
            );
        }
    }
    Ok(())
}

fn export_terrain_command(
    project: &Path,
    output: &Path,
    nodata: Option<f32>,
    json: bool,
) -> Result<()> {
    let loaded = vf_io::load_project(project)?;
    let report = vf_io::export_terrain(
        output,
        &loaded.heightfield,
        &loaded.project.meta.name,
        nodata,
    )?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": report.path,
                "format": report.format,
                "width": report.width,
                "height": report.height,
                "spacing_m": report.spacing_m,
                "warnings": report.warnings,
            }))?
        );
    } else {
        println!(
            "exported {}x{} terrain to {} ({})",
            report.width,
            report.height,
            report.path.display(),
            report.format
        );
        for warning in &report.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}

fn run_blender(command: BlenderCommand) -> Result<()> {
    match command {
        BlenderCommand::Export {
            project,
            out,
            subdivision,
            no_water,
            no_camera,
            no_sun,
            json: json_output,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let options = vf_export::ExportOptions {
                subdivision: subdivision.unwrap_or(loaded.project.render.subdivision.max(1) as u32),
                water: !no_water,
                camera: !no_camera,
                sun: !no_sun,
                ..vf_export::ExportOptions::default()
            };
            let report =
                vf_export::export_project(&loaded.project, &loaded.heightfield, &out, &options)
                    .map_err(|e| anyhow::anyhow!("blender export failed: {e}"))?;
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "glb": report.glb,
                        "sidecar": report.sidecar,
                        "heightmap": report.heightmap,
                        "albedo": report.albedo,
                        "normal": report.normal,
                        "masks": report.masks,
                        "vertices": report.vertices,
                        "triangles": report.triangles,
                        "materials": report.materials,
                    }))?
                );
            } else {
                println!(
                    "exported {} vertices, {} triangles, {} materials -> {}",
                    report.vertices,
                    report.triangles,
                    report.materials,
                    report.glb.display()
                );
                println!("sidecar:   {}", report.sidecar.display());
                println!("heightmap: {}", report.heightmap.display());
                println!(
                    "textures:  {} {} {}",
                    report.albedo.display(),
                    report.normal.display(),
                    report.masks.display()
                );
                println!(
                    "import:    blender --background --python bridge/blender/vistaforge_import.py -- {} {} --render render.png",
                    report.glb.display(),
                    report.sidecar.display()
                );
            }
            Ok(())
        }
        BlenderCommand::Render {
            project,
            image,
            out_dir,
            blender,
            script,
            samples,
            width,
            height,
            gpu,
            smart_uv,
            remesh,
            save_blend,
        } => {
            let loaded = vf_io::load_project(&project)?;
            let dir = out_dir.unwrap_or_else(|| image.with_extension("vfblend"));
            let options = vf_export::ExportOptions {
                subdivision: loaded.project.render.subdivision.max(1) as u32,
                ..vf_export::ExportOptions::default()
            };
            let report =
                vf_export::export_project(&loaded.project, &loaded.heightfield, &dir, &options)
                    .map_err(|e| anyhow::anyhow!("blender export failed: {e}"))?;
            let script =
                script.unwrap_or_else(|| PathBuf::from("bridge/blender/vistaforge_import.py"));
            if !script.exists() {
                bail!(
                    "Blender import script not found at {} (pass --script)",
                    script.display()
                );
            }
            let blender = blender.unwrap_or_else(|| PathBuf::from("blender"));
            let mut command = std::process::Command::new(&blender);
            command
                .arg("--background")
                .arg("--python")
                .arg(&script)
                .arg("--")
                .arg(&report.glb)
                .arg(&report.sidecar)
                .arg("--render")
                .arg(&image)
                .arg("--samples")
                .arg(samples.to_string());
            if let Some(width) = width {
                command.arg("--width").arg(width.to_string());
            }
            if let Some(height) = height {
                command.arg("--height").arg(height.to_string());
            }
            if gpu {
                command.arg("--gpu");
            }
            if smart_uv {
                command.arg("--smart-uv");
            }
            if remesh {
                command.arg("--remesh");
            }
            if let Some(save_blend) = save_blend {
                command.arg("--save-blend").arg(save_blend);
            }
            println!("running {} ...", blender.display());
            let status = command
                .status()
                .with_context(|| format!("cannot run Blender at {}", blender.display()))?;
            if !status.success() {
                bail!("Blender exited with {status}");
            }
            if !image.exists() {
                bail!("Blender finished but {} was not written", image.display());
            }
            println!("rendered {} -> {}", project.display(), image.display());
            Ok(())
        }
    }
}

fn print_project_info(path: &Path, json_output: bool) -> Result<()> {
    let loaded = vf_io::load_project(path)?;
    let p = &loaded.project;
    // The saved heightfield is the base terrain; the stack is live. Report the
    // stats the renderer will actually use.
    let shaped;
    let stats = if p.modifiers.active().next().is_some() {
        shaped = vf_modifiers::apply(&loaded.heightfield, &p.modifiers, p.seed)?;
        shaped.stats()
    } else {
        loaded.heightfield.stats()
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "file": path,
                "id": p.id.to_string(),
                "name": p.meta.name,
                "seed": p.seed.0,
                "source": p.source.kind(),
                "dimensions": [p.terrain.width, p.terrain.height],
                "spacing": [p.terrain.spacing_x, p.terrain.spacing_y],
                "elevation": {
                    "min": stats.min,
                    "max": stats.max,
                    "mean": stats.mean,
                    "std_dev": stats.std_dev,
                    "relief": stats.relief(),
                },
                "camera": {
                    "position": p.camera.position.to_array(),
                    "target": p.camera.target.to_array(),
                    "fov": p.camera.fov_y_deg,
                },
                "sun": {
                    "azimuth_from_south": p.sun.azimuth_deg_from_south,
                    "declination": p.sun.declination_deg,
                },
                "water": {
                    "enabled": p.water.enabled,
                    "sea_level": p.water.sea_level,
                },
                "render": {
                    "width": p.render.width,
                    "height": p.render.height,
                    "ssaa": p.render.ssaa,
                    "subdivision": p.render.subdivision,
                    "cliff_warp": p.render.cliff_warp,
                },
                "detail": {
                    "enabled": p.detail.enabled,
                    "profile": p.detail.profile,
                    "strength": p.detail.strength,
                    "scale_m": p.detail.scale_m,
                    "amplitude_m": p.detail.amplitude_m,
                    "heterogeneity": p.detail.heterogeneity,
                    "focus_enabled": p.detail.focus_enabled,
                },
                "modifiers": {
                    "count": p.modifiers.items.len(),
                    "enabled": p.modifiers.active().count(),
                    "summary": p.modifiers.summary(),
                },
            }))?
        );
    } else {
        println!("file:        {}", path.display());
        println!("name:        {}", p.meta.name);
        println!("id:          {}", p.id);
        println!("schema:      {}", p.schema_version);
        println!("seed:        {}", p.seed);
        println!("source:      {}", p.source.kind());
        println!(
            "dimensions:  {}x{} (spacing {:.1} x {:.1} m)",
            p.terrain.width, p.terrain.height, p.terrain.spacing_x, p.terrain.spacing_y
        );
        println!(
            "elevation:   {:.1}..{:.1} m (mean {:.1}, relief {:.1})",
            stats.min,
            stats.max,
            stats.mean,
            stats.relief()
        );
        println!(
            "camera:      pos ({:.0}, {:.0}, {:.0}) -> target ({:.0}, {:.0}, {:.0}), fov {:.0}°",
            p.camera.position.x,
            p.camera.position.y,
            p.camera.position.z,
            p.camera.target.x,
            p.camera.target.y,
            p.camera.target.z,
            p.camera.fov_y_deg
        );
        println!(
            "sun:         azimuth {:.0}° from south, declination {:.0}°, {:.0} K",
            p.sun.azimuth_deg_from_south, p.sun.declination_deg, p.sun.color_k
        );
        println!(
            "water:       {} (sea level {:.1} m)",
            if p.water.enabled { "on" } else { "off" },
            p.water.sea_level
        );
        println!(
            "render:      {}x{} ssaa {} · subdivision {}× · cliff warp {:.2}",
            p.render.width,
            p.render.height,
            p.render.ssaa,
            p.render.subdivision,
            p.render.cliff_warp
        );
        println!(
            "detail:      {} (profile {}, scale {:.1} m, amplitude {:.1} m, hétérogénéité {:.2})",
            if p.detail.enabled { "on" } else { "off" },
            p.detail.profile,
            p.detail.scale_m,
            p.detail.amplitude_m,
            p.detail.heterogeneity
        );
        println!(
            "modifiers:   {} entries ({} active): {}",
            p.modifiers.items.len(),
            p.modifiers.active().count(),
            p.modifiers.summary()
        );
    }
    Ok(())
}

fn read_dem(path: &Path) -> Result<DemFile> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    DemFile::parse(&bytes).with_context(|| format!("invalid DEM {}", path.display()))
}

fn parse_vec3(value: &str, what: &str) -> Result<glam::DVec3> {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        bail!("{what} must be formatted as x,y,z");
    }
    let mut numbers = [0.0f64; 3];
    for (i, part) in parts.iter().enumerate() {
        numbers[i] = part
            .parse()
            .with_context(|| format!("invalid {what} component '{part}'"))?;
    }
    Ok(glam::DVec3::new(numbers[0], numbers[1], numbers[2]))
}

fn parse_pair(value: &str, what: &str) -> Result<(f32, f32)> {
    let (a, b) = value
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("{what} must be formatted as a,b"))?;
    Ok((
        a.trim()
            .parse()
            .with_context(|| format!("invalid {what} first value"))?,
        b.trim()
            .parse()
            .with_context(|| format!("invalid {what} second value"))?,
    ))
}

fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "landscape".into()
    } else {
        trimmed
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}
