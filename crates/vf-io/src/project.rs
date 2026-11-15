//! `.vfproj` project container.
//!
//! A project is a ZIP archive with a human-readable RON manifest and raw
//! binary terrain assets:
//!
//! ```text
//! project.ron
//! terrain/heightmap.r32   # f32 little-endian, width*height
//! terrain/mask.u8         # optional, 0/1 per sample
//! preview.png             # optional
//! ```

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use vf_core::{HeightField, Project, SCHEMA_VERSION, TerrainMeta};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::image_io::RgbaImage;
use crate::{IoError, Result};

const MANIFEST: &str = "project.ron";
const HEIGHTMAP: &str = "terrain/heightmap.r32";
const MASK: &str = "terrain/mask.u8";
const PREVIEW: &str = "preview.png";

#[derive(Debug, Clone)]
pub struct LoadedProject {
    pub project: Project,
    pub heightfield: HeightField,
    pub preview: Option<RgbaImage>,
    pub path: PathBuf,
}

/// Save a project and its heightfield. The manifest's terrain metadata is
/// refreshed from the heightfield before writing.
pub fn save_project(
    project: &Project,
    heightfield: &HeightField,
    path: &Path,
    preview: Option<&RgbaImage>,
) -> Result<()> {
    let mut project = project.clone();
    project.terrain = TerrainMeta::from_heightfield(heightfield);
    project.validate().map_err(IoError::Invalid)?;

    let manifest = ron::ser::to_string_pretty(
        &project,
        ron::ser::PrettyConfig::default()
            .struct_names(false)
            .enumerate_arrays(false),
    )
    .map_err(|e| IoError::Ron(e.to_string()))?;

    let mut height_bytes = Vec::with_capacity(heightfield.sample_count() * 4);
    for &value in heightfield.heights() {
        height_bytes.extend_from_slice(&value.to_le_bytes());
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("vfproj.tmp");
    {
        let file = std::fs::File::create(&tmp)?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        zip.start_file(MANIFEST, options)?;
        zip.write_all(manifest.as_bytes())?;
        zip.start_file(HEIGHTMAP, options)?;
        zip.write_all(&height_bytes)?;
        if let Some(mask) = heightfield.mask() {
            zip.start_file(MASK, options)?;
            let bytes: Vec<u8> = mask.iter().map(|&valid| u8::from(valid)).collect();
            zip.write_all(&bytes)?;
        }
        if let Some(preview) = preview {
            let mut png = Vec::new();
            {
                let buffer = image::RgbaImage::from_raw(
                    preview.width,
                    preview.height,
                    preview.pixels.clone(),
                )
                .ok_or_else(|| IoError::Invalid("invalid preview dimensions".into()))?;
                let mut cursor = std::io::Cursor::new(&mut png);
                image::DynamicImage::ImageRgba8(buffer)
                    .write_to(&mut cursor, image::ImageFormat::Png)?;
            }
            zip.start_file(PREVIEW, options)?;
            zip.write_all(&png)?;
        }
        zip.finish()?;
    }

    // `rename` replaces atomically on Unix. Windows refuses to replace an
    // existing file, so only there do we remove the target first; the fully
    // written temporary file still protects against a crash mid-write.
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Bring an older manifest up to the current schema. Missing fields were
/// already filled by `#[serde(default)]`; only the version needs a nudge.
fn migrate(project: &mut Project) {
    if project.schema_version < SCHEMA_VERSION {
        project.schema_version = SCHEMA_VERSION;
    }
    // Repair animation fields that older builds could write as zero. The rig
    // is validated on load, so a stale duration must not make a whole project
    // unreadable.
    if !project.rig.duration_s.is_finite() || project.rig.duration_s < 0.1 {
        project.rig.duration_s = 8.0;
    }
    if !(1..=120).contains(&project.rig.fps) {
        project.rig.fps = 24;
    }
    if !project.rig.optics.fov_y_deg.is_finite()
        || !(1.0..=170.0).contains(&project.rig.optics.fov_y_deg)
    {
        project.rig.optics.fov_y_deg = 50.0;
    }
}

/// Load a project and its heightfield.
pub fn load_project(path: &Path) -> Result<LoadedProject> {
    let file = std::fs::File::open(path)?;
    let mut archive = ZipArchive::new(file)?;

    let manifest = {
        let mut entry = archive
            .by_name(MANIFEST)
            .map_err(|_| IoError::Invalid(format!("{MANIFEST} missing from archive")))?;
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        text
    };
    let mut project: Project = ron::from_str(&manifest).map_err(|e| IoError::Ron(e.to_string()))?;
    // Older projects predate the geology, materials, erosion and flight
    // fields; the serde defaults fill them in.
    migrate(&mut project);
    if project.terrain.width < 2 || project.terrain.height < 2 {
        return Err(IoError::Invalid(
            "project manifest has no terrain dimensions".into(),
        ));
    }

    let expected = project.terrain.width * project.terrain.height;
    let mut height_bytes = Vec::with_capacity(expected * 4);
    {
        let mut entry = archive
            .by_name(HEIGHTMAP)
            .map_err(|_| IoError::Invalid(format!("{HEIGHTMAP} missing from archive")))?;
        entry.read_to_end(&mut height_bytes)?;
    }
    if height_bytes.len() != expected * 4 {
        return Err(IoError::Invalid(format!(
            "heightmap has {} bytes, expected {} for {}x{}",
            height_bytes.len(),
            expected * 4,
            project.terrain.width,
            project.terrain.height
        )));
    }
    let heights: Vec<f32> = height_bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    if heights.iter().any(|v| !v.is_finite()) {
        return Err(IoError::Invalid(
            "heightmap contains non-finite values".into(),
        ));
    }

    let mut heightfield = HeightField::new(
        project.terrain.width,
        project.terrain.height,
        project.terrain.spacing_x,
        project.terrain.spacing_y,
        heights,
    )?;
    heightfield.set_origin(project.terrain.origin[0], project.terrain.origin[1]);
    heightfield.set_nodata(project.terrain.nodata);

    if project.terrain.has_mask {
        let mut mask_bytes = Vec::new();
        {
            let mut entry = archive
                .by_name(MASK)
                .map_err(|_| IoError::Invalid(format!("{MASK} missing from archive")))?;
            entry.read_to_end(&mut mask_bytes)?;
        }
        if mask_bytes.len() != expected {
            return Err(IoError::Invalid(format!(
                "mask has {} bytes, expected {expected}",
                mask_bytes.len()
            )));
        }
        heightfield.set_mask(Some(mask_bytes.iter().map(|&b| b != 0).collect()))?;
    }

    let preview = match archive.by_name(PREVIEW) {
        Ok(mut entry) => {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            let image = image::load_from_memory(&bytes)?.to_rgba8();
            Some(RgbaImage::new(
                image.width(),
                image.height(),
                image.into_raw(),
            )?)
        }
        Err(_) => None,
    };

    project.validate().map_err(IoError::Invalid)?;
    Ok(LoadedProject {
        project,
        heightfield,
        preview,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{Project, Seed};

    fn sample_project() -> (Project, HeightField) {
        let mut project = Project::new("Round trip", Seed::new(12), 33);
        let hf = HeightField::new(
            33,
            33,
            30.0,
            30.0,
            (0..33 * 33).map(|i| (i % 300) as f32).collect(),
        )
        .unwrap();
        project.sync_terrain(&hf);
        (project, hf)
    }

    #[test]
    fn legacy_schema_version_is_migrated() {
        let mut project = Project::new("Old", Seed::new(1), 33);
        project.schema_version = 1;
        migrate(&mut project);
        assert_eq!(project.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn project_round_trip() {
        let (project, hf) = sample_project();
        let dir = std::env::temp_dir().join(format!("vfproj-test-{}", std::process::id()));
        let path = dir.join("scene.vfproj");
        save_project(&project, &hf, &path, None).unwrap();
        let loaded = load_project(&path).unwrap();
        assert_eq!(loaded.project.meta.name, "Round trip");
        assert_eq!(loaded.heightfield.heights(), hf.heights());
        assert_eq!(loaded.project.terrain.width, 33);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migrate_repairs_zero_duration_and_fps() {
        let mut project = Project::new("Old", Seed::new(1), 33);
        project.rig.duration_s = 0.0;
        project.rig.fps = 0;
        project.rig.optics.fov_y_deg = 0.0;
        migrate(&mut project);
        assert!(project.rig.duration_s >= 0.1);
        assert!((1..=120).contains(&project.rig.fps));
        assert!(project.rig.validate().is_ok());
    }

    #[test]
    fn failed_save_preserves_the_previous_file() {
        let (project, hf) = sample_project();
        let dir = std::env::temp_dir().join(format!("vfproj-atomic-{}", std::process::id()));
        let path = dir.join("scene.vfproj");
        save_project(&project, &hf, &path, None).unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut broken = project.clone();
        broken.rig.duration_s = 0.0; // invalid
        assert!(save_project(&broken, &hf, &path, None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupted_project_returns_an_error_instead_of_panicking() {
        let dir = std::env::temp_dir().join(format!("vfproj-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.vfproj");
        std::fs::write(&path, b"not a zip").unwrap();
        assert!(load_project(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_with_mask_and_preview() {
        let (project, mut hf) = sample_project();
        let mut mask = vec![true; 33 * 33];
        mask[5] = false;
        hf.set_mask(Some(mask)).unwrap();
        let preview = RgbaImage::new(2, 2, vec![255; 16]).unwrap();
        let dir = std::env::temp_dir().join(format!("vfproj-mask-{}", std::process::id()));
        let path = dir.join("scene.vfproj");
        save_project(&project, &hf, &path, Some(&preview)).unwrap();
        let loaded = load_project(&path).unwrap();
        assert_eq!(loaded.heightfield.mask().unwrap().len(), 33 * 33);
        assert!(!loaded.heightfield.is_valid(5, 0));
        assert!(loaded.preview.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
