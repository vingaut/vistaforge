//! Background work and progress reporting.
//!
//! Operations that can take seconds (terrain generation, project I/O, DEM
//! import/export, glTF export) must never run on the UI thread: on Windows a
//! blocked message pump makes the window report "not responding". The small
//! runner below keeps the heavy part on a worker thread, publishes a progress
//! value, and requests a repaint so the gauge moves even when the user is not
//! touching the mouse.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use vf_core::{HeightField, Project};
use vf_export::{AnimationReport, ImportedAnimation};
use vf_io::{ImportedTerrain, LoadedProject, TerrainExportReport};

const UNKNOWN: u32 = u32::MAX;

/// Shared, thread-safe progress of one operation.
pub struct Progress {
    phase: Mutex<String>,
    /// Permille in `0..=1000`, or [`UNKNOWN`].
    value: AtomicU32,
    ctx: egui::Context,
    started: Instant,
}

impl Progress {
    pub fn new(ctx: egui::Context, phase: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            phase: Mutex::new(phase.into()),
            value: AtomicU32::new(UNKNOWN),
            ctx,
            started: Instant::now(),
        })
    }

    /// Set a known progress in `0.0..=1.0`.
    pub fn set(&self, fraction: f32) {
        let permille = (fraction.clamp(0.0, 1.0) * 1000.0).round() as u32;
        self.value.store(permille, Ordering::Relaxed);
        self.ctx.request_repaint();
    }

    /// Switch back to an animated bar when the worker cannot estimate a
    /// meaningful percentage (compression, a single long erosion pass…).
    pub fn set_indeterminate(&self) {
        self.value.store(UNKNOWN, Ordering::Relaxed);
        self.ctx.request_repaint();
    }

    /// Change the phase label without changing the fraction.
    pub fn set_phase(&self, phase: impl Into<String>) {
        if let Ok(mut current) = self.phase.lock() {
            *current = phase.into();
        }
        self.ctx.request_repaint();
    }

    pub fn phase(&self) -> String {
        self.phase
            .lock()
            .map_or_else(|_| "Traitement…".to_string(), |phase| phase.clone())
    }

    pub fn fraction(&self) -> Option<f32> {
        match self.value.load(Ordering::Relaxed) {
            UNKNOWN => None,
            value => Some(value as f32 / 1000.0),
        }
    }

    /// Fraction to draw when the operation cannot report real progress. It
    /// rises asymptotically so the bar visibly moves without pretending to
    /// know the remaining time.
    pub fn display_fraction(&self) -> f32 {
        self.fraction().unwrap_or_else(|| {
            let seconds = self.started.elapsed().as_secs_f32();
            (1.0 - (-seconds / 9.0).exp()).clamp(0.0, 0.9)
        })
    }
}

/// What the operation is, so panels can show the right spinner without
/// parsing a translated title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Generate,
    LoadProject,
    SaveProject,
    SaveRender,
    ImportTerrain,
    ExportTerrain,
    ComposeWorld,
    BakeModifiers,
    Densify,
    Blender,
    AnimationExport,
    AnimationImport,
}

/// Payload produced by a background operation.
pub enum TaskResult {
    ProjectLoaded {
        loaded: LoadedProject,
    },
    ProjectSaved {
        path: PathBuf,
    },
    RenderSaved {
        path: PathBuf,
    },
    TerrainGenerated {
        project: Project,
        heightfield: HeightField,
        size: u32,
        seed: u64,
        elapsed_ms: u128,
    },
    TerrainImported {
        path: PathBuf,
        imported: ImportedTerrain,
    },
    TerrainExported {
        report: TerrainExportReport,
    },
    TerrainComposed {
        heightfield: HeightField,
    },
    ModifiersBaked {
        heightfield: HeightField,
    },
    TerrainDensified {
        heightfield: HeightField,
        old_width: usize,
        new_width: usize,
    },
    BlenderExported {
        report: vf_export::ExportReport,
        note: Option<String>,
    },
    AnimationExported {
        report: AnimationReport,
    },
    AnimationImported {
        imported: ImportedAnimation,
    },
    Failed(String),
}

pub enum Poll {
    Pending,
    Finished(Box<TaskResult>),
    Disconnected,
}

/// One operation on a worker thread.
pub struct Task {
    pub kind: TaskKind,
    pub title: String,
    pub progress: Arc<Progress>,
    receiver: Receiver<TaskResult>,
    started: Instant,
}

impl Task {
    pub fn spawn<F>(ctx: &egui::Context, kind: TaskKind, title: impl Into<String>, work: F) -> Self
    where
        F: FnOnce(Arc<Progress>) -> TaskResult + Send + 'static,
    {
        let title = title.into();
        let progress = Progress::new(ctx.clone(), title.clone());
        let (sender, receiver) = mpsc::channel();
        let worker_progress = Arc::clone(&progress);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = work(Arc::clone(&worker_progress));
            let _ = sender.send(result);
            repaint.request_repaint();
        });
        Self {
            kind,
            title,
            progress,
            receiver,
            started: Instant::now(),
        }
    }

    pub fn poll(&self, ctx: &egui::Context) -> Poll {
        match self.receiver.try_recv() {
            Ok(result) => Poll::Finished(Box::new(result)),
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                Poll::Pending
            }
            Err(TryRecvError::Disconnected) => Poll::Disconnected,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_publishes_progress_and_returns_its_result() {
        let ctx = egui::Context::default();
        let task = Task::spawn(&ctx, TaskKind::Generate, "test", |progress| {
            progress.set_phase("phase de test");
            progress.set(0.25);
            TaskResult::Failed("terminé".into())
        });
        for _ in 0..400 {
            match task.poll(&ctx) {
                Poll::Finished(result) => match *result {
                    TaskResult::Failed(message) => {
                        assert_eq!(message, "terminé");
                        assert_eq!(task.progress.phase(), "phase de test");
                        assert_eq!(task.progress.fraction(), Some(0.25));
                        return;
                    }
                    _ => panic!("unexpected task result"),
                },
                Poll::Disconnected => panic!("worker disconnected"),
                Poll::Pending => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("the task did not finish in time");
    }

    #[test]
    fn unknown_progress_still_moves_the_gauge() {
        let ctx = egui::Context::default();
        let progress = Progress::new(ctx, "lent");
        let first = progress.display_fraction();
        std::thread::sleep(Duration::from_millis(20));
        let second = progress.display_fraction();
        assert!(second > first);
        assert!(second <= 0.9);
        assert!(progress.fraction().is_none());
    }
}
