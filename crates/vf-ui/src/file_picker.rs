//! Small in-app file browser.
//!
//! A native dialog would pull a new platform dependency for a desktop app that
//! otherwise stays self-contained. This picker lists a directory, filters the
//! extensions relevant to the current action and returns a path; all file
//! access still goes through `std::fs`.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickTarget {
    Project,
    ImportTerrain,
    ExportTerrain,
    Animation,
    BlenderDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickMode {
    OpenFile,
    SaveFile,
    Directory,
}

pub struct PickResult {
    pub target: PickTarget,
    pub path: PathBuf,
}

pub struct FilePicker {
    open: bool,
    target: PickTarget,
    mode: PickMode,
    dir: PathBuf,
    name: String,
    filters: Vec<String>,
    error: String,
}

impl Default for FilePicker {
    fn default() -> Self {
        Self {
            open: false,
            target: PickTarget::Project,
            mode: PickMode::OpenFile,
            dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            name: String::new(),
            filters: Vec::new(),
            error: String::new(),
        }
    }
}

impl FilePicker {
    pub fn open(&mut self, target: PickTarget, mode: PickMode, current: &str, filters: &[&str]) {
        let current = PathBuf::from(current.trim());
        let (dir, name) = if current.is_dir() {
            (current.clone(), String::new())
        } else {
            let dir = current
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or_else(
                    || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    Path::to_path_buf,
                );
            let name = current
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            (dir, name)
        };
        self.target = target;
        self.mode = mode;
        self.dir = dir;
        self.name = name;
        self.filters = filters
            .iter()
            .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
            .collect();
        self.error.clear();
        self.open = true;
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<PickResult> {
        if !self.open {
            return None;
        }
        let mut result = None;
        let mut open = self.open;
        egui::Window::new(match self.mode {
            PickMode::OpenFile => "Ouvrir un fichier",
            PickMode::SaveFile => "Enregistrer sous",
            PickMode::Directory => "Choisir un dossier",
        })
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([660.0, 460.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⌂").on_hover_text("Dossier personnel").clicked()
                    && let Some(home) = std::env::var_os("HOME")
                {
                    self.dir = PathBuf::from(home);
                }
                if ui.button("↑").on_hover_text("Dossier parent").clicked()
                    && let Some(parent) = self.dir.parent()
                {
                    self.dir = parent.to_path_buf();
                }
                let mut folder = self.dir.display().to_string();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut folder)
                            .desired_width(ui.available_width() - 8.0),
                    )
                    .changed()
                {
                    let candidate = PathBuf::from(folder);
                    if candidate.is_dir() {
                        self.dir = candidate;
                    }
                }
            });
            ui.separator();
            let entries = list_dir(&self.dir, &self.filters);
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if ui
                        .selectable_label(false, RichText::new("..").monospace())
                        .clicked()
                        && let Some(parent) = self.dir.parent()
                    {
                        self.dir = parent.to_path_buf();
                    }
                    for entry in entries {
                        let label = if entry.is_dir {
                            format!("📁 {}", entry.name)
                        } else {
                            format!("📄 {}", entry.name)
                        };
                        let selected = !entry.is_dir
                            && self
                                .name
                                .eq_ignore_ascii_case(&entry.path.to_string_lossy());
                        let response = ui.selectable_label(selected, label);
                        if response.clicked() {
                            if entry.is_dir {
                                self.dir = entry.path.clone();
                            } else {
                                self.name = entry.name.clone();
                            }
                        }
                        if response.double_clicked() && !entry.is_dir {
                            result = Some(PickResult {
                                target: self.target,
                                path: entry.path,
                            });
                        }
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Nom");
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .desired_width(ui.available_width() - 150.0),
                );
            });
            if !self.error.is_empty() {
                ui.label(RichText::new(&self.error).color(Color32::from_rgb(255, 140, 120)));
            }
            ui.horizontal(|ui| {
                let confirm = match self.mode {
                    PickMode::OpenFile => "Ouvrir",
                    PickMode::SaveFile => "Enregistrer",
                    PickMode::Directory => "Choisir ce dossier",
                };
                if ui.button(confirm).clicked() {
                    let path = match self.mode {
                        PickMode::Directory => Some(self.dir.clone()),
                        PickMode::OpenFile | PickMode::SaveFile => {
                            let name = self.name.trim();
                            if name.is_empty() {
                                self.error = "Choisissez un fichier ou saisissez un nom.".into();
                                None
                            } else {
                                let candidate = if Path::new(name).is_absolute() {
                                    PathBuf::from(name)
                                } else {
                                    self.dir.join(name)
                                };
                                if self.mode == PickMode::OpenFile && !candidate.exists() {
                                    self.error = format!("{} n'existe pas.", candidate.display());
                                    None
                                } else {
                                    Some(candidate)
                                }
                            }
                        }
                    };
                    if let Some(path) = path {
                        result = Some(PickResult {
                            target: self.target,
                            path,
                        });
                    }
                }
                if ui.button("Annuler").clicked() {
                    self.open = false;
                }
            });
        });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        self.open = open;
        if result.is_some() {
            self.open = false;
        }
        result
    }
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

fn list_dir(dir: &Path, filters: &[String]) -> Vec<Entry> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = path.is_dir();
        if !is_dir && !filters.is_empty() {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !filters.iter().any(|filter| filter == &extension) {
                continue;
            }
        }
        entries.push(Entry { name, path, is_dir });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_keeps_only_requested_extensions() {
        let dir = std::env::temp_dir().join(format!("vf-picker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.tif"), b"x").unwrap();
        std::fs::write(dir.join("b.png"), b"x").unwrap();
        std::fs::write(dir.join("c.txt"), b"x").unwrap();
        let entries = list_dir(&dir, &["tif".to_string(), "png".to_string()]);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"a.tif"));
        assert!(names.contains(&"b.png"));
        assert!(!names.contains(&"c.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
