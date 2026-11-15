use std::path::PathBuf;

const USAGE: &str = "\
VistaForge desktop application

Usage:
  vistaforge-app [PROJECT.vfproj]

Options:
  -h, --help       Print this help and exit
  -V, --version    Print the version and exit

Environment:
  VISTAFORGE_RENDERER=glow   Force the OpenGL UI renderer and the CPU preview
                             instead of the default wgpu/Vulkan viewport.
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
    let initial = args
        .into_iter()
        .find(|arg| !arg.starts_with('-'))
        .map(PathBuf::from);
    vf_ui::run(initial).map_err(|error| anyhow::anyhow!("GUI error: {error}"))
}
