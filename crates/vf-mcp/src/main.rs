use std::io::{stdin, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "vistaforge-mcp",
    version,
    about = "VistaForge MCP server (stdio transport)"
)]
struct Args {
    /// Root directory exposed to the MCP client.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    #[arg(long)]
    verbose: bool,
    /// Worker threads (0 = all logical cores). Also VISTAFORGE_THREADS.
    #[arg(long)]
    threads: Option<usize>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let threads = args.threads.unwrap_or_else(|| {
        std::env::var("VISTAFORGE_THREADS")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    });
    if let Err(error) = vf_core::compute::configure_threads(threads) {
        eprintln!("warning: cannot configure compute pool: {error}");
    }
    if args.verbose {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();
    }
    let workspace = match std::fs::create_dir_all(&args.workspace)
        .and_then(|()| args.workspace.canonicalize())
    {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "error: cannot prepare workspace '{}': {error}",
                args.workspace.display()
            );
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "vistaforge-mcp {} (workspace {})",
        env!("CARGO_PKG_VERSION"),
        workspace.display()
    );
    match vf_mcp::serve(stdin().lock(), stdout().lock(), workspace) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: MCP transport failed: {error}");
            ExitCode::FAILURE
        }
    }
}
