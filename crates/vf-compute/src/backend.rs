//! Runtime selection of the compute engine.
//!
//! The preferences store the choice in [`vf_core::ComputeSettings`]; this
//! module holds the live state the kernels read. It is deliberately independent
//! from the settings file so the CLI and the tests can switch backends without
//! touching the user profile.
//!
//! A GPU request that cannot be satisfied (no adapter, shader failure, device
//! lost) never aborts an operation: the kernels fall back to the CPU path and
//! [`status`] reports why, so the preferences panel can explain the fallback.

use std::sync::{OnceLock, RwLock};

pub use vf_core::compute::ComputeBackend;

/// Live backend choice.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendState {
    pub backend: ComputeBackend,
    /// Adapter key (`backend|name`) or name for [`ComputeBackend::Gpu`].
    /// Empty means "first available adapter".
    pub adapter: String,
    /// Adapters for [`ComputeBackend::GpuAll`]. Empty means "every adapter".
    pub adapters: Vec<String>,
}

impl BackendState {
    pub fn cpu() -> Self {
        Self::default()
    }

    pub fn gpu(adapter: impl Into<String>) -> Self {
        Self {
            backend: ComputeBackend::Gpu,
            adapter: adapter.into(),
            adapters: Vec::new(),
        }
    }

    pub fn gpu_all(adapters: Vec<String>) -> Self {
        Self {
            backend: ComputeBackend::GpuAll,
            adapter: String::new(),
            adapters,
        }
    }

    /// Read the durable choice.
    pub fn from_settings(settings: &vf_core::ComputeSettings) -> Self {
        Self {
            backend: settings.compute,
            adapter: settings.compute_adapter.clone(),
            adapters: settings.compute_adapters.clone(),
        }
    }

    /// Write the choice back into the durable settings before saving them.
    pub fn write_to(&self, settings: &mut vf_core::ComputeSettings) {
        settings.compute = self.backend;
        settings.compute_adapter = self.adapter.clone();
        settings.compute_adapters = self.adapters.clone();
    }

    /// Adapter keys this state asks for; empty means "every detected adapter".
    pub fn adapter_keys(&self) -> Vec<String> {
        match self.backend {
            ComputeBackend::Cpu => Vec::new(),
            ComputeBackend::Gpu => {
                if self.adapter.is_empty() {
                    Vec::new()
                } else {
                    vec![self.adapter.clone()]
                }
            }
            ComputeBackend::GpuAll => self.adapters.clone(),
        }
    }
}

/// What the preferences panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeStatus {
    pub backend: ComputeBackend,
    /// Threads the CPU path will use.
    pub cpu_threads: usize,
    /// True when the GPU part of the selected backend is usable.
    pub gpu_available: bool,
    /// Adapter label that the GPU path will use, or the fallback reason.
    pub gpu_label: String,
    /// Every adapter detected, for the picker.
    pub adapters: Vec<String>,
    /// `(key, label)` of every adapter detected.
    pub adapter_choices: Vec<(String, String)>,
}

impl ComputeStatus {
    /// One-line summary, e.g. `GPU · NVIDIA GeForce RTX 5070 · Vulkan`.
    pub fn summary(&self) -> String {
        match self.backend {
            ComputeBackend::Cpu => format!("CPU · {} threads", self.cpu_threads),
            ComputeBackend::Gpu | ComputeBackend::GpuAll => {
                if self.gpu_available {
                    format!("{} · {}", self.backend.label(), self.gpu_label)
                } else {
                    format!("CPU (repli) · {}", self.gpu_label)
                }
            }
        }
    }
}

fn state_cell() -> &'static RwLock<BackendState> {
    static STATE: OnceLock<RwLock<BackendState>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(BackendState::cpu()))
}

/// Current backend choice.
pub fn backend_state() -> BackendState {
    state_cell()
        .read()
        .map(|state| state.clone())
        .unwrap_or_default()
}

/// Select the backend. Cheap: devices are opened lazily by the first kernel.
pub fn set_backend(state: BackendState) {
    if let Ok(mut guard) = state_cell().write() {
        if *guard != state {
            gpu_backend::invalidate();
        }
        *guard = state;
    }
}

/// Select the backend from persisted settings and report the status.
pub fn configure(settings: &vf_core::ComputeSettings) -> ComputeStatus {
    set_backend(BackendState::from_settings(settings));
    status()
}

/// Status of the live selection.
pub fn status() -> ComputeStatus {
    status_for(&backend_state())
}

/// Status of an arbitrary state, without changing the live selection.
pub fn status_for(state: &BackendState) -> ComputeStatus {
    let cpu_threads = vf_core::compute::current_threads();
    let choices = detected_adapters();
    let adapters: Vec<String> = choices.iter().map(|(_, label)| label.clone()).collect();
    let (gpu_available, gpu_label) = match state.backend {
        ComputeBackend::Cpu => (
            !choices.is_empty(),
            if choices.is_empty() {
                "aucun adaptateur de calcul détecté".to_string()
            } else {
                format!("{} adaptateur(s) de calcul détecté(s)", choices.len())
            },
        ),
        ComputeBackend::Gpu => match gpu_backend::open(&state.adapter, false) {
            Ok(contexts) => (
                true,
                contexts
                    .first()
                    .map(|context| context.adapter.label())
                    .unwrap_or_else(|| "adaptateur GPU".to_string()),
            ),
            Err(error) => (false, error),
        },
        ComputeBackend::GpuAll => match gpu_backend::open_set(&state.adapters) {
            Ok(contexts) => (
                true,
                contexts
                    .iter()
                    .map(|context| context.adapter.label())
                    .collect::<Vec<_>>()
                    .join(" + "),
            ),
            Err(error) => (false, error),
        },
    };
    ComputeStatus {
        backend: state.backend,
        cpu_threads,
        gpu_available,
        gpu_label,
        adapters,
        adapter_choices: choices,
    }
}

/// `(key, label)` of every compute adapter, empty when built without `gpu`.
pub fn detected_adapters() -> Vec<(String, String)> {
    gpu_backend::detected()
}

/// Force the next GPU kernel call to reopen its devices.
pub fn invalidate_gpu() {
    gpu_backend::invalidate();
}

#[cfg(feature = "gpu")]
mod gpu_backend {
    pub use crate::gpu::{detected, invalidate, open, open_set};
}

#[cfg(not(feature = "gpu"))]
mod gpu_backend {
    use std::sync::Arc;

    /// Placeholder so the dispatch code can stay feature-agnostic.
    pub struct GpuContext;

    impl GpuContext {
        pub fn label(&self) -> String {
            "build sans support GPU".to_string()
        }
    }

    pub fn detected() -> Vec<(String, String)> {
        Vec::new()
    }

    pub fn invalidate() {}

    pub fn open(_adapter: &str, _all: bool) -> Result<Vec<Arc<GpuContext>>, String> {
        Err("build sans support GPU (feature `gpu`) : calcul sur le CPU".to_string())
    }

    pub fn open_set(_adapters: &[String]) -> Result<Vec<Arc<GpuContext>>, String> {
        Err("build sans support GPU (feature `gpu`) : calcul sur le CPU".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_the_cpu() {
        let state = BackendState::cpu();
        assert_eq!(state.backend, ComputeBackend::Cpu);
        assert!(state.adapter.is_empty());
        assert!(state.adapter_keys().is_empty());
    }

    #[test]
    fn state_round_trips_through_settings() {
        let mut settings = vf_core::ComputeSettings::default();
        let state = BackendState::gpu_all(vec!["vulkan|GPU A".into(), "vulkan|GPU B".into()]);
        state.write_to(&mut settings);
        assert_eq!(BackendState::from_settings(&settings), state);
        assert_eq!(settings.compute, ComputeBackend::GpuAll);
    }

    #[test]
    fn status_summary_mentions_the_cpu_threads() {
        let status = status_for(&BackendState::cpu());
        assert!(status.summary().starts_with("CPU"));
        assert!(status.summary().contains("threads"));
    }

    #[test]
    fn a_gpu_state_without_a_device_falls_back_cleanly() {
        // On a machine without Vulkan/DX12 the status must say so instead of
        // panicking; the kernels then use the CPU path.
        let status = status_for(&BackendState::gpu("inexistant"));
        if !status.gpu_available {
            assert!(status.summary().starts_with("CPU (repli)"));
        }
    }
}
