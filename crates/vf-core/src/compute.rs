//! Global compute configuration: CPU worker pool and GPU preference.
//!
//! Every parallel operation in VistaForge runs through [`install`], which uses
//! one shared [`rayon::ThreadPool`]. The pool can be rebuilt at runtime with a
//! different number of threads, so the desktop UI can change the CPU budget
//! without restarting.
//!
//! [`ComputeSettings::compute`] additionally selects *which engine* runs the
//! geometric transformations: the shared CPU pool, one GPU adapter, or every
//! adapter at once. The wgpu side lives in `vf-compute`; this module only stores
//! the durable preference and the environment overrides.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};

/// Preferred GPU power profile when no explicit adapter is selected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuPower {
    /// Prefer the discrete/high-performance adapter.
    #[default]
    HighPerformance,
    /// Prefer the integrated/low-power adapter.
    LowPower,
}

/// Engine used by the geometric transformations (normals, smoothing, detail
/// displacement, transforms).
///
/// Rendering always stays on the CPU rasteriser: this setting only covers mesh
/// geometry work, which is what sculpting and remeshing spend their time on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeBackend {
    /// Shared rayon pool, sized by [`ComputeSettings::cpu_threads`].
    #[default]
    Cpu,
    /// One GPU adapter ([`ComputeSettings::compute_adapter`]).
    Gpu,
    /// Every detected adapter, with the vertex range split between them.
    GpuAll,
}

impl ComputeBackend {
    pub const ALL: [ComputeBackend; 3] = [Self::Cpu, Self::Gpu, Self::GpuAll];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Gpu => "GPU (un adaptateur)",
            Self::GpuAll => "GPU (tous les adaptateurs)",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Cpu => {
                "Pool de threads partagé, nombre de cœurs réglable ci-dessous. Chemin de référence, déterministe."
            }
            Self::Gpu => {
                "Calcule les transformations géométriques sur un adaptateur wgpu. Repli CPU si aucun GPU n'est disponible."
            }
            Self::GpuAll => {
                "Répartit les sommets entre tous les adaptateurs détectés. Utile sur une machine multi-GPU."
            }
        }
    }

    pub fn uses_gpu(self) -> bool {
        !matches!(self, Self::Cpu)
    }

    /// Parse an environment value (`cpu`, `gpu`, `all`/`gpu-all`).
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" | "off" => Some(Self::Cpu),
            "gpu" | "one" => Some(Self::Gpu),
            "all" | "gpu-all" | "gpu_all" | "multi" => Some(Self::GpuAll),
            _ => None,
        }
    }
}

impl GpuPower {
    pub fn label(self) -> &'static str {
        match self {
            Self::HighPerformance => "Performance",
            Self::LowPower => "Économie d'énergie",
        }
    }

    pub fn from_label(label: &str) -> Self {
        if label.contains("Économie") || label.contains("conomie") {
            Self::LowPower
        } else {
            Self::HighPerformance
        }
    }
}

/// Persistent compute preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ComputeSettings {
    /// Worker threads. `0` means "all logical cores".
    pub cpu_threads: usize,
    /// Exact adapter key (`backend|name`), empty for automatic selection.
    pub gpu_adapter: String,
    pub gpu_power: GpuPower,
    /// Use wgpu for the 3D viewport. When false the app uses the CPU preview.
    pub gpu_enabled: bool,
    /// Engine used by the geometric transformations.
    pub compute: ComputeBackend,
    /// Adapter used by [`ComputeBackend::Gpu`], empty for the first available.
    pub compute_adapter: String,
    /// Adapters used by [`ComputeBackend::GpuAll`], empty for all detected.
    pub compute_adapters: Vec<String>,
}

impl Default for ComputeSettings {
    fn default() -> Self {
        Self {
            cpu_threads: 0,
            gpu_adapter: String::new(),
            gpu_power: GpuPower::HighPerformance,
            gpu_enabled: true,
            compute: ComputeBackend::Cpu,
            compute_adapter: String::new(),
            compute_adapters: Vec::new(),
        }
    }
}

impl ComputeSettings {
    /// Logical cores reported by the OS.
    pub fn logical_cores() -> usize {
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
    }

    /// Number of threads this configuration asks for.
    pub fn resolved_threads(&self) -> usize {
        Self::clamp_threads(self.cpu_threads)
    }

    /// Clamp a thread count to `1..=logical`, with `0` meaning automatic.
    pub fn clamp_threads(threads: usize) -> usize {
        let max = Self::logical_cores();
        if threads == 0 {
            max
        } else {
            threads.clamp(1, max.max(1))
        }
    }

    /// Human-readable CPU budget.
    pub fn cpu_label(&self) -> String {
        let threads = self.resolved_threads();
        if self.cpu_threads == 0 {
            format!("{threads} threads (tous les cœurs logiques)")
        } else {
            format!("{threads} threads sur {}", Self::logical_cores())
        }
    }

    /// Apply environment overrides. Useful for scripts and CI.
    pub fn with_env_overrides(mut self) -> Self {
        if let Ok(value) = std::env::var("VISTAFORGE_THREADS")
            && let Ok(threads) = value.trim().parse::<usize>()
        {
            self.cpu_threads = threads;
        }
        if let Ok(value) = std::env::var("VISTAFORGE_GPU") {
            let value = value.trim();
            if value.eq_ignore_ascii_case("off")
                || value.eq_ignore_ascii_case("cpu")
                || value.eq_ignore_ascii_case("none")
            {
                self.gpu_enabled = false;
            } else if !value.is_empty() && !value.eq_ignore_ascii_case("auto") {
                self.gpu_enabled = true;
                self.gpu_adapter = value.to_string();
            }
        }
        if let Ok(value) = std::env::var("VISTAFORGE_COMPUTE")
            && let Some(backend) = ComputeBackend::from_env_value(&value)
        {
            self.compute = backend;
        }
        if let Ok(value) = std::env::var("VISTAFORGE_COMPUTE_GPU") {
            let value = value.trim();
            if !value.is_empty() {
                self.compute_adapter = value.to_string();
                self.compute_adapters = vec![value.to_string()];
            }
        }
        self
    }
}

fn settings_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("VistaForge").join("compute.ron"))
    }
    #[cfg(target_os = "macos")]
    {
        let base = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(base)
                .join("Library")
                .join("Application Support")
                .join("VistaForge")
                .join("compute.ron"),
        )
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("vistaforge").join("compute.ron"))
    }
}

/// Load persisted settings, or the defaults when nothing is stored.
pub fn load_settings() -> ComputeSettings {
    let Some(path) = settings_path() else {
        return ComputeSettings::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return ComputeSettings::default();
    };
    ron::from_str(&text).unwrap_or_default()
}

/// Persist settings. Failures are silent: preferences must never block work.
pub fn save_settings(settings: &ComputeSettings) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = ron::ser::to_string_pretty(settings, ron::ser::PrettyConfig::default()) {
        let _ = std::fs::write(path, text);
    }
}

fn build_pool(threads: usize) -> Result<ThreadPool, String> {
    ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .stack_size(8 * 1024 * 1024)
        .thread_name(|index| format!("vistaforge-{index}"))
        .build()
        .map_err(|error| error.to_string())
}

fn pool_cell() -> &'static RwLock<Arc<ThreadPool>> {
    static POOL: OnceLock<RwLock<Arc<ThreadPool>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = ComputeSettings::resolved_threads(&ComputeSettings::default());
        let pool = build_pool(threads).expect("worker pool");
        RwLock::new(Arc::new(pool))
    })
}

/// Threads currently available to parallel jobs.
pub fn current_threads() -> usize {
    pool_cell()
        .read()
        .map(|pool| pool.current_num_threads())
        .unwrap_or(1)
}

/// Rebuild the worker pool with `threads` workers (`0` = all logical cores).
/// Returns the number of threads actually used.
pub fn configure_threads(threads: usize) -> Result<usize, String> {
    let threads = ComputeSettings::clamp_threads(threads);
    let pool = build_pool(threads)?;
    let mut guard = pool_cell()
        .write()
        .map_err(|_| "verrou du pool de calcul empoisonné".to_string())?;
    *guard = Arc::new(pool);
    Ok(threads)
}

/// Build the global pool from a settings value.
pub fn configure(settings: &ComputeSettings) -> Result<usize, String> {
    configure_threads(settings.cpu_threads)
}

/// Run `job` on the shared worker pool.
///
/// Rayon's parallel iterators created inside the closure use this pool, so
/// every call site automatically follows the chosen thread count. Calls made
/// from a worker of the same pool run inline.
pub fn install<R: Send>(job: impl FnOnce() -> R + Send) -> R {
    let pool = match pool_cell().read() {
        Ok(pool) => Arc::clone(&pool),
        Err(_) => return job(),
    };
    pool.install(job)
}

/// Worker-pool name shown in diagnostics (`vistaforge-0` …).
pub fn worker_name(index: usize) -> String {
    format!("vistaforge-{index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_count_is_clamped_to_logical_cores() {
        let max = ComputeSettings::logical_cores();
        assert_eq!(ComputeSettings::clamp_threads(0), max);
        assert_eq!(ComputeSettings::clamp_threads(1), 1);
        assert_eq!(ComputeSettings::clamp_threads(max + 100), max);
    }

    #[test]
    fn settings_round_trip_through_ron() {
        let settings = ComputeSettings {
            cpu_threads: 3,
            gpu_adapter: "vulkan|Test GPU".into(),
            gpu_power: GpuPower::LowPower,
            gpu_enabled: false,
            compute: ComputeBackend::GpuAll,
            compute_adapter: "vulkan|Second GPU".into(),
            compute_adapters: vec!["vulkan|Test GPU".into(), "vulkan|Second GPU".into()],
        };
        let text = ron::ser::to_string(&settings).expect("serialize");
        let decoded: ComputeSettings = ron::from_str(&text).expect("deserialize");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn missing_compute_fields_fall_back_to_the_cpu() {
        // A file written by a version without the compute backend must still
        // load: the geometry then stays on the CPU path.
        let text =
            "(cpu_threads: 2, gpu_adapter: \"\", gpu_power: high_performance, gpu_enabled: true)";
        let decoded: ComputeSettings = ron::from_str(text).expect("deserialize");
        assert_eq!(decoded.compute, ComputeBackend::Cpu);
        assert!(decoded.compute_adapter.is_empty());
        assert!(decoded.compute_adapters.is_empty());
    }

    #[test]
    fn backend_parses_environment_values() {
        assert_eq!(
            ComputeBackend::from_env_value("cpu"),
            Some(ComputeBackend::Cpu)
        );
        assert_eq!(
            ComputeBackend::from_env_value("GPU"),
            Some(ComputeBackend::Gpu)
        );
        assert_eq!(
            ComputeBackend::from_env_value("all"),
            Some(ComputeBackend::GpuAll)
        );
        assert_eq!(ComputeBackend::from_env_value("peut-être"), None);
        assert!(ComputeBackend::Gpu.uses_gpu());
        assert!(!ComputeBackend::Cpu.uses_gpu());
    }

    #[test]
    fn install_uses_the_worker_pool() {
        let threads = install(rayon::current_num_threads);
        assert_eq!(threads, current_threads());
    }
}
