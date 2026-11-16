//! Configurable undo/redo history for the desktop editor.
//!
//! The editor exposes hundreds of controls, so the history does not try to
//! describe every mutation. It keeps a *baseline* of the last committed state
//! and compares the live project with it. A slider drag therefore becomes one
//! undo step once the pointer settles, and a discrete operation can force a
//! commit before it starts.
//!
//! Terrain samples are shared with [`std::sync::Arc`]: a project-only edit
//! costs one `Project` clone, while a real heightfield edit keeps the previous
//! samples alive only for as long as the memory budget allows. Sculpt strokes
//! keep their compact region undo instead of a full terrain snapshot.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use vf_core::{Camera, HeightField, Project};
use vf_terrain::brush::BrushUndo;

const DEFAULT_LABEL: &str = "Modification";

/// User-tunable history limits. Values are normalised when applied so the
/// editor can never ask for an absurd allocation or a zero-step history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    pub enabled: bool,
    /// Maximum number of undo steps. Redo steps are not counted.
    pub max_steps: usize,
    /// Approximate upper bound for the memory retained by the history.
    pub memory_limit_mb: usize,
    /// Slider drags and close successive edits are grouped until the user has
    /// been idle for this long.
    pub merge_delay_ms: u64,
    /// Keep one compact entry per sculpt stroke.
    pub include_sculpt: bool,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_steps: 30,
            memory_limit_mb: 512,
            merge_delay_ms: 400,
            include_sculpt: true,
        }
    }
}

impl HistorySettings {
    pub fn normalized(mut self) -> Self {
        self.max_steps = self.max_steps.clamp(1, 200);
        self.memory_limit_mb = self.memory_limit_mb.clamp(16, 16_384);
        self.merge_delay_ms = self.merge_delay_ms.min(5_000);
        self
    }

    pub fn merge_delay(&self) -> Duration {
        Duration::from_millis(self.merge_delay_ms)
    }

    pub fn memory_limit_bytes(&self) -> usize {
        self.memory_limit_mb.saturating_mul(1_048_576)
    }
}

#[derive(Debug, Clone)]
struct Snapshot {
    project: Project,
    heightfield: Arc<HeightField>,
    label: String,
}

#[derive(Debug, Clone)]
enum Entry {
    Snapshot(Box<Snapshot>),
    Sculpt { undo: BrushUndo, label: String },
}

impl Entry {
    fn label(&self) -> &str {
        match self {
            Self::Snapshot(snapshot) => &snapshot.label,
            Self::Sculpt { label, .. } => label,
        }
    }
}

/// Summary shown in the preferences and in the undo tooltips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HistoryStats {
    pub undo_steps: usize,
    pub redo_steps: usize,
    pub bytes: usize,
    pub limit_bytes: usize,
}

pub struct History {
    settings: HistorySettings,
    undo: VecDeque<Entry>,
    redo: VecDeque<Entry>,
    /// Committed state before the current edit gesture. `None` while a sculpt
    /// stroke is in progress: the compact `BrushUndo` will represent it.
    baseline: Option<Snapshot>,
    pending: bool,
    last_change: Option<Instant>,
    pending_label: Option<String>,
    sculpting: bool,
}

impl History {
    pub fn new(
        settings: HistorySettings,
        project: &Project,
        heightfield: &Arc<HeightField>,
    ) -> Self {
        let settings = settings.normalized();
        Self {
            settings,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            baseline: Some(Snapshot {
                project: project.clone(),
                heightfield: Arc::clone(heightfield),
                label: "État initial".into(),
            }),
            pending: false,
            last_change: None,
            pending_label: None,
            sculpting: false,
        }
    }

    pub fn settings(&self) -> &HistorySettings {
        &self.settings
    }

    /// Apply new preferences. Lowering the limits prunes immediately; turning
    /// the history off drops the retained states (the live project is kept).
    pub fn set_settings(
        &mut self,
        settings: HistorySettings,
        project: &Project,
        heightfield: &Arc<HeightField>,
    ) {
        self.flush(project, heightfield);
        self.settings = settings.normalized();
        if !self.settings.enabled {
            self.undo.clear();
            self.redo.clear();
        }
        self.baseline = Some(Snapshot {
            project: project.clone(),
            heightfield: Arc::clone(heightfield),
            label: "État courant".into(),
        });
        self.pending = false;
        self.last_change = None;
        self.pending_label = None;
        self.prune();
    }

    /// Drop all steps and start from the current state (new/open project).
    pub fn clear(&mut self, project: &Project, heightfield: &Arc<HeightField>) {
        self.undo.clear();
        self.redo.clear();
        self.baseline = Some(Snapshot {
            project: project.clone(),
            heightfield: Arc::clone(heightfield),
            label: "État initial".into(),
        });
        self.pending = false;
        self.last_change = None;
        self.pending_label = None;
        self.sculpting = false;
    }

    /// Suggest a label for the next recorded step. A more specific label wins
    /// over the generic fallback set by `mark_dirty`.
    pub fn label(&mut self, label: impl Into<String>) {
        let label = label.into();
        if self.pending {
            if self
                .pending_label
                .as_deref()
                .is_none_or(|current| current == DEFAULT_LABEL)
            {
                self.pending_label = Some(label);
            }
        } else {
            self.pending_label = Some(label);
        }
    }

    /// Compare the live state with the baseline and remember that an edit
    /// happened. Returns `true` when a meaningful change was detected.
    pub fn observe(&mut self, project: &Project, heightfield: &Arc<HeightField>) -> bool {
        if !self.settings.enabled || self.sculpting {
            return false;
        }
        let Some(baseline) = self.baseline.as_ref() else {
            return false;
        };
        let project_changed = !projects_equivalent(&baseline.project, project);
        let terrain_changed = !Arc::ptr_eq(&baseline.heightfield, heightfield);
        if !project_changed && !terrain_changed {
            return false;
        }
        if !self.pending {
            self.pending = true;
            self.pending_label
                .get_or_insert_with(|| DEFAULT_LABEL.to_string());
        }
        self.last_change = Some(Instant::now());
        true
    }

    /// Observe and commit when the edit gesture has settled. Call once per
    /// frame (start and end) so a drag is grouped into one step.
    pub fn tick(&mut self, project: &Project, heightfield: &Arc<HeightField>) {
        self.observe(project, heightfield);
        if self.pending
            && self
                .last_change
                .is_some_and(|last| last.elapsed() >= self.settings.merge_delay())
        {
            self.commit(project, heightfield);
        }
    }

    /// Commit a pending edit immediately. Returns `true` when a step was
    /// pushed onto the undo stack.
    pub fn flush(&mut self, project: &Project, heightfield: &Arc<HeightField>) -> bool {
        if !self.pending {
            return false;
        }
        self.commit(project, heightfield)
    }

    fn commit(&mut self, project: &Project, heightfield: &Arc<HeightField>) -> bool {
        let Some(baseline) = self.baseline.take() else {
            // No baseline: a sculpt gesture is being handled separately.
            self.baseline = Some(Snapshot {
                project: project.clone(),
                heightfield: Arc::clone(heightfield),
                label: "État courant".into(),
            });
            self.pending = false;
            self.last_change = None;
            self.pending_label = None;
            return false;
        };
        let mut label = self
            .pending_label
            .take()
            .unwrap_or_else(|| DEFAULT_LABEL.to_string());
        if baseline.label == "État initial" || baseline.label == "État courant" {
            label = label.trim().to_string();
        }
        self.undo
            .push_back(Entry::Snapshot(Box::new(Snapshot { label, ..baseline })));
        self.redo.clear();
        self.baseline = Some(Snapshot {
            project: project.clone(),
            heightfield: Arc::clone(heightfield),
            label: "État courant".into(),
        });
        self.pending = false;
        self.last_change = None;
        self.prune();
        true
    }

    /// Start a sculpture stroke. The compact region undo is pushed by
    /// [`Self::push_sculpt`] on release, so the full terrain baseline can be
    /// released and `Arc::make_mut` mutates in place.
    pub fn begin_sculpt(&mut self, project: &Project, heightfield: &Arc<HeightField>) {
        self.flush(project, heightfield);
        self.sculpting = true;
        self.pending = false;
        self.last_change = None;
        self.pending_label = None;
        if self.settings.enabled && self.settings.include_sculpt {
            // The compact BrushUndo will represent this gesture, so the full
            // terrain baseline can be released and `Arc::make_mut` mutates in
            // place.
            self.baseline = None;
        }
    }

    /// Finish a sculpture stroke with its compact undo entry.
    pub fn push_sculpt(
        &mut self,
        undo: BrushUndo,
        project: &Project,
        heightfield: &Arc<HeightField>,
    ) {
        self.end_sculpt(project, heightfield);
        if !self.settings.enabled || !self.settings.include_sculpt {
            return;
        }
        let label = undo.label.clone();
        self.undo.push_back(Entry::Sculpt { undo, label });
        self.redo.clear();
        self.prune();
    }

    /// End a stroke that did not touch any sample (or whose history is
    /// disabled). Without this, the missing baseline would stop observation
    /// for every later edit.
    pub fn end_sculpt(&mut self, project: &Project, heightfield: &Arc<HeightField>) {
        self.sculpting = false;
        self.baseline = Some(Snapshot {
            project: project.clone(),
            heightfield: Arc::clone(heightfield),
            label: "État courant".into(),
        });
        self.pending = false;
        self.last_change = None;
        self.pending_label = None;
    }

    pub fn can_undo(&self) -> bool {
        self.settings.enabled && !self.sculpting && !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.settings.enabled && !self.sculpting && !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(Entry::label)
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.back().map(Entry::label)
    }

    pub fn stats(&self) -> HistoryStats {
        HistoryStats {
            undo_steps: self.undo.len(),
            redo_steps: self.redo.len(),
            bytes: self.memory_bytes(),
            limit_bytes: self.settings.memory_limit_bytes(),
        }
    }

    /// Undo one step. Returns the label of the restored action.
    pub fn undo(
        &mut self,
        project: &mut Project,
        heightfield: &mut Arc<HeightField>,
    ) -> Option<String> {
        if !self.settings.enabled || self.sculpting {
            return None;
        }
        self.flush(project, heightfield);
        let entry = self.undo.pop_back()?;
        let label = entry.label().to_string();
        match entry {
            Entry::Snapshot(snapshot) => {
                let current = Box::new(Snapshot {
                    project: project.clone(),
                    heightfield: Arc::clone(heightfield),
                    label: label.clone(),
                });
                self.redo.push_back(Entry::Snapshot(current));
                *project = snapshot.project.clone();
                *heightfield = Arc::clone(&snapshot.heightfield);
                self.baseline = Some(*snapshot);
            }
            Entry::Sculpt { undo, label } => {
                // Release the baseline Arc before patching the region, so the
                // compact undo does not degenerate into a full terrain clone.
                self.baseline = None;
                undo.undo(Arc::make_mut(heightfield));
                project.mark_modified();
                self.redo.push_back(Entry::Sculpt {
                    undo,
                    label: label.clone(),
                });
                self.baseline = Some(Snapshot {
                    project: project.clone(),
                    heightfield: Arc::clone(heightfield),
                    label: "État courant".into(),
                });
            }
        }
        self.pending = false;
        self.last_change = None;
        self.pending_label = None;
        self.sculpting = false;
        self.prune();
        Some(label)
    }

    /// Redo one step. Returns the label of the reapplied action.
    pub fn redo(
        &mut self,
        project: &mut Project,
        heightfield: &mut Arc<HeightField>,
    ) -> Option<String> {
        if !self.settings.enabled || self.sculpting {
            return None;
        }
        self.flush(project, heightfield);
        let entry = self.redo.pop_back()?;
        let label = entry.label().to_string();
        match entry {
            Entry::Snapshot(snapshot) => {
                let current = Box::new(Snapshot {
                    project: project.clone(),
                    heightfield: Arc::clone(heightfield),
                    label: label.clone(),
                });
                self.undo.push_back(Entry::Snapshot(current));
                *project = snapshot.project.clone();
                *heightfield = Arc::clone(&snapshot.heightfield);
                self.baseline = Some(*snapshot);
            }
            Entry::Sculpt { undo, label } => {
                self.baseline = None;
                undo.redo(Arc::make_mut(heightfield));
                project.mark_modified();
                self.undo.push_back(Entry::Sculpt {
                    undo,
                    label: label.clone(),
                });
                self.baseline = Some(Snapshot {
                    project: project.clone(),
                    heightfield: Arc::clone(heightfield),
                    label: "État courant".into(),
                });
            }
        }
        self.pending = false;
        self.last_change = None;
        self.pending_label = None;
        self.sculpting = false;
        self.prune();
        Some(label)
    }

    fn prune(&mut self) {
        while self.undo.len() > self.settings.max_steps {
            self.undo.pop_front();
        }
        let limit = self.settings.memory_limit_bytes();
        // Keep at least one step: a single very large heightfield snapshot is
        // still more useful than an undo stack that silently disappears.
        while self.undo.len() > 1 && self.memory_bytes() > limit {
            self.undo.pop_front();
        }
        // Redo entries cannot outnumber undo entries, but a huge redo stack
        // must not retain memory either.
        while self.redo.len() > self.settings.max_steps {
            self.redo.pop_front();
        }
        while !self.redo.is_empty() && self.memory_bytes() > limit && self.redo.len() > 1 {
            self.redo.pop_front();
        }
    }

    fn memory_bytes(&self) -> usize {
        fn add_snapshot(snapshot: &Snapshot, seen: &mut HashSet<usize>, total: &mut usize) {
            *total += project_bytes(&snapshot.project);
            let key = Arc::as_ptr(&snapshot.heightfield) as *const () as usize;
            if seen.insert(key) {
                *total += heightfield_bytes(&snapshot.heightfield);
            }
        }
        let mut seen = HashSet::new();
        let mut total = 0usize;
        if let Some(baseline) = &self.baseline {
            add_snapshot(baseline, &mut seen, &mut total);
        }
        for entry in self.undo.iter().chain(self.redo.iter()) {
            match entry {
                Entry::Snapshot(snapshot) => add_snapshot(snapshot, &mut seen, &mut total),
                Entry::Sculpt { undo, .. } => total += undo.bytes() + 96,
            }
        }
        total
    }
}

/// Camera moves are navigation, not document edits. The modified timestamp is
/// also ignored so a `mark_modified` call with identical settings does not
/// create a step.
fn projects_equivalent(left: &Project, right: &Project) -> bool {
    if left == right {
        return true;
    }
    let mut left = left.clone();
    let mut right = right.clone();
    // Orbiting is navigation, not a document edit. The field of view, on the
    // other hand, changes the framing of the exported image and stays in the
    // comparison.
    left.camera = comparable_camera(&left.camera);
    right.camera = comparable_camera(&right.camera);
    left.meta.modified_unix = 0;
    right.meta.modified_unix = 0;
    left == right
}

fn comparable_camera(camera: &Camera) -> Camera {
    Camera {
        fov_y_deg: camera.fov_y_deg,
        ..Camera::default()
    }
}

fn heightfield_bytes(heightfield: &HeightField) -> usize {
    let mask = heightfield.mask().map_or(0, <[bool]>::len);
    heightfield.sample_count() * std::mem::size_of::<f32>() + mask + 256
}

fn project_bytes(project: &Project) -> usize {
    std::mem::size_of::<Project>()
        + project.meta.name.len()
        + project.meta.author.len()
        + project.meta.description.len()
        + project.modifiers.items.len() * 512
        + (project.rig.eye.keys.len() + project.rig.target.path.keys.len()) * 192
}

/// Per-user preferences file. The undo limits are the only editor setting
/// that benefits from persistence; the help stays off at every launch.
fn preferences_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("VistaForge").join("ui.ron"))
    }
    #[cfg(target_os = "macos")]
    {
        let base = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(base)
                .join("Library")
                .join("Application Support")
                .join("VistaForge")
                .join("ui.ron"),
        )
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("vistaforge").join("ui.ron"))
    }
}

/// Load persisted undo settings. A missing or unreadable file simply returns
/// `None`, leaving the defaults in place.
pub fn load_settings() -> Option<HistorySettings> {
    let path = preferences_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    ron::from_str(&text).ok()
}

/// Persist undo settings. Failures are deliberately silent: preferences are a
/// convenience and must never prevent the editor from working.
pub fn save_settings(settings: &HistorySettings) {
    let Some(path) = preferences_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = ron::ser::to_string_pretty(settings, ron::ser::PrettyConfig::default()) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::Seed;
    use vf_terrain::brush::BrushRegion;

    fn scene(size: usize) -> (Project, Arc<HeightField>) {
        let project = Project::new("History test", Seed::new(7), size as u32);
        let heightfield =
            Arc::new(HeightField::filled(size, size, 10.0, 0.0).expect("heightfield"));
        (project, heightfield)
    }

    fn settings(delay: u64) -> HistorySettings {
        HistorySettings {
            enabled: true,
            max_steps: 10,
            memory_limit_mb: 256,
            merge_delay_ms: delay,
            include_sculpt: true,
        }
    }

    #[test]
    fn commits_a_project_edit_and_undoes_it() {
        let (mut project, mut heightfield) = scene(8);
        let mut history = History::new(settings(0), &project, &heightfield);
        project.meta.name = "Edited".into();
        project.mark_modified();
        assert!(history.observe(&project, &heightfield));
        history.tick(&project, &heightfield);
        assert!(history.can_undo());
        assert_eq!(history.undo_label(), Some(DEFAULT_LABEL));
        let label = history.undo(&mut project, &mut heightfield);
        assert_eq!(label.as_deref(), Some(DEFAULT_LABEL));
        assert_eq!(project.meta.name, "History test");
        assert!(history.can_redo());
        history.redo(&mut project, &mut heightfield);
        assert_eq!(project.meta.name, "Edited");
    }

    #[test]
    fn camera_navigation_is_not_a_history_step() {
        let (mut project, heightfield) = scene(8);
        let mut history = History::new(settings(0), &project, &heightfield);
        project.camera.position.x += 25.0;
        history.tick(&project, &heightfield);
        assert!(!history.can_undo());
    }

    #[test]
    fn field_of_view_is_a_document_edit() {
        let (mut project, mut heightfield) = scene(8);
        let mut history = History::new(settings(0), &project, &heightfield);
        project.camera.fov_y_deg = 72.0;
        history.tick(&project, &heightfield);
        assert!(history.can_undo());
        history.undo(&mut project, &mut heightfield);
        assert!((project.camera.fov_y_deg - 50.0).abs() < 1e-6);
    }

    #[test]
    fn grouped_drag_is_one_step() {
        let (mut project, mut heightfield) = scene(8);
        let mut history = History::new(settings(60), &project, &heightfield);
        project.camera.position.x += 1.0; // ignored
        project.meta.name = "A".into();
        history.tick(&project, &heightfield);
        project.meta.name = "AB".into();
        history.tick(&project, &heightfield);
        assert_eq!(history.stats().undo_steps, 0);
        history.flush(&project, &heightfield);
        assert_eq!(history.stats().undo_steps, 1);
        history.undo(&mut project, &mut heightfield);
        assert_eq!(project.meta.name, "History test");
    }

    #[test]
    fn depth_limit_drops_the_oldest_steps() {
        let (mut project, mut heightfield) = scene(8);
        let mut config = settings(0);
        config.max_steps = 2;
        let mut history = History::new(config, &project, &heightfield);
        for index in 0..5 {
            project.meta.name = format!("edit {index}");
            history.tick(&project, &heightfield);
            history.flush(&project, &heightfield);
        }
        assert_eq!(history.stats().undo_steps, 2);
        history.undo(&mut project, &mut heightfield);
        assert_eq!(project.meta.name, "edit 3");
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut project, mut heightfield) = scene(8);
        let mut history = History::new(settings(0), &project, &heightfield);
        project.meta.name = "one".into();
        history.tick(&project, &heightfield);
        history.flush(&project, &heightfield);
        history.undo(&mut project, &mut heightfield);
        assert!(history.can_redo());
        project.meta.name = "two".into();
        history.tick(&project, &heightfield);
        history.flush(&project, &heightfield);
        assert!(!history.can_redo());
    }

    #[test]
    fn sculpt_region_undo_and_redo() {
        let (mut project, mut heightfield) = scene(8);
        // The test mutates the Arc in place; the history entry stays compact.
        let mut history = History::new(settings(0), &project, &heightfield);
        history.begin_sculpt(&project, &heightfield);
        Arc::make_mut(&mut heightfield).heights_mut()[9] = 42.0;
        let undo = BrushUndo {
            label: "Sculpter".into(),
            region: BrushRegion {
                x0: 1,
                y0: 1,
                x1: 2,
                y1: 2,
            },
            before: vec![0.0],
            after: vec![42.0],
        };
        history.push_sculpt(undo, &project, &heightfield);
        assert!(history.can_undo());
        assert_eq!(history.undo_label(), Some("Sculpter"));
        history.undo(&mut project, &mut heightfield);
        assert_eq!(heightfield.heights()[9], 0.0);
        history.redo(&mut project, &mut heightfield);
        assert_eq!(heightfield.heights()[9], 42.0);
    }

    #[test]
    fn settings_round_trip_through_the_preferences_format() {
        let settings = HistorySettings {
            enabled: false,
            max_steps: 7,
            memory_limit_mb: 64,
            merge_delay_ms: 120,
            include_sculpt: false,
        };
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("serialize");
        let decoded: HistorySettings = ron::from_str(&text).expect("deserialize");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn empty_or_disabled_sculpt_does_not_leave_the_history_stuck() {
        let (mut project, mut heightfield) = scene(8);
        let mut config = settings(0);
        config.include_sculpt = false;
        let mut history = History::new(config, &project, &heightfield);
        // A stroke is not recorded, but observation must resume afterwards.
        history.begin_sculpt(&project, &heightfield);
        Arc::make_mut(&mut heightfield).heights_mut()[9] = 3.0;
        assert!(!history.observe(&project, &heightfield));
        history.end_sculpt(&project, &heightfield);
        assert!(!history.can_undo());
        project.meta.name = "after stroke".into();
        assert!(history.observe(&project, &heightfield));
        history.flush(&project, &heightfield);
        assert!(history.can_undo());
    }

    #[test]
    fn memory_budget_prunes_terrain_snapshots() {
        let (mut project, mut heightfield) = scene(64);
        let mut config = settings(0);
        config.max_steps = 20;
        config.memory_limit_mb = 16;
        let mut history = History::new(config, &project, &heightfield);
        for index in 0..8 {
            heightfield = Arc::new((*heightfield).clone());
            project.meta.name = format!("edit {index}");
            history.tick(&project, &heightfield);
            history.flush(&project, &heightfield);
        }
        let stats = history.stats();
        assert!(stats.undo_steps <= 8);
        assert!(stats.bytes <= stats.limit_bytes || stats.undo_steps == 1);
    }
}
