use std::path::PathBuf;

const USAGE: &str = "\
VistaForge desktop application

Usage:
  vistaforge-app [PROJECT.vfproj] [--threads N] [--compute ENGINE] [--compute-gpu NAME]

Options:
  -h, --help           Print this help and exit
  -V, --version        Print the version and exit
      --threads N      Worker threads (0 = all logical cores)
      --compute ENGINE Geometry engine: cpu, gpu or all (every GPU)
      --compute-gpu NAME
                       Adapter name or key for --compute gpu

Environment:
  VISTAFORGE_RENDERER=glow   Force the OpenGL UI renderer and the CPU preview
                             instead of the default wgpu/Vulkan viewport.
  VISTAFORGE_THREADS=N       Worker threads (0 or unset = all logical cores).
  VISTAFORGE_GPU=auto|off|<name>
                             GPU adapter for the 3D preview.
  VISTAFORGE_COMPUTE=cpu|gpu|all
                             Geometry engine (normals, smoothing, detail,
                             transforms, sculpting).
  VISTAFORGE_COMPUTE_GPU=NAME
                             Adapter for the geometry engine.
";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        println!("vistaforge-app {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut initial = None;
    let mut threads = None;
    let mut engine = vf_ui::ComputeOverride::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--threads" {
            threads = iter.next().and_then(|value| value.parse().ok());
        } else if let Some(value) = arg.strip_prefix("--threads=") {
            threads = value.parse().ok();
        } else if arg == "--compute" {
            engine.backend = iter.next();
        } else if let Some(value) = arg.strip_prefix("--compute=") {
            engine.backend = Some(value.to_string());
        } else if arg == "--compute-gpu" {
            engine.adapter = iter.next();
        } else if let Some(value) = arg.strip_prefix("--compute-gpu=") {
            engine.adapter = Some(value.to_string());
        } else if !arg.starts_with('-') && initial.is_none() {
            initial = Some(PathBuf::from(arg));
        }
    }
    vf_ui::run(initial, threads, engine).map_err(|error| anyhow::anyhow!("GUI error: {error}"))
}
