//! Tolerant ESRI ASCII Grid import/export.
//!
//! ESRI grids store the north row first; VistaForge heightfields store the
//! south row first, so rows are reversed on both import and export.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use vf_core::HeightField;

use crate::{IoError, Result};

const KNOWN_KEYS: &[&str] = &[
    "ncols",
    "nrows",
    "xllcorner",
    "yllcorner",
    "xllcenter",
    "yllcenter",
    "cellsize",
    "nodata_value",
];

/// Parse an ASCII grid from a string.
pub fn parse_ascii_grid(text: &str) -> Result<HeightField> {
    let mut header: HashMap<String, f64> = HashMap::new();
    let mut data_tokens: Vec<&str> = Vec::new();
    let mut in_data = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if !in_data {
            let mut parts = line.split_whitespace();
            if let Some(first) = parts.next() {
                let key = first.to_ascii_lowercase();
                if KNOWN_KEYS.contains(&key.as_str())
                    && let Some(value) = parts.next()
                {
                    let value = value.trim_end_matches(',');
                    let parsed = value.parse::<f64>().map_err(|_| {
                        IoError::Invalid(format!("invalid ASCII grid header value: {line}"))
                    })?;
                    header.insert(key, parsed);
                    continue;
                }
            }
            in_data = true;
        }
        data_tokens.extend(line.split_whitespace());
    }

    if data_tokens.is_empty() {
        return Err(IoError::Invalid("ASCII grid contains no samples".into()));
    }

    let values: Vec<f64> = data_tokens
        .iter()
        .map(|token| {
            token
                .trim_end_matches(',')
                .parse::<f64>()
                .map_err(|_| IoError::Invalid(format!("invalid ASCII grid sample: {token}")))
        })
        .collect::<Result<_>>()?;

    let (width, height) = if let (Some(&w), Some(&h)) = (header.get("ncols"), header.get("nrows")) {
        let w = w as usize;
        let h = h as usize;
        if w < 2 || h < 2 {
            return Err(IoError::Invalid(format!(
                "ASCII grid dimensions too small: {w}x{h}"
            )));
        }
        if values.len() != w * h {
            return Err(IoError::Invalid(format!(
                "ASCII grid has {} samples, expected {} for {w}x{h}",
                values.len(),
                w * h
            )));
        }
        (w, h)
    } else {
        // Matrix mode: use the first line's token count as width.
        let first_line = text
            .lines()
            .find(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with('#') && !line.starts_with(';')
            })
            .ok_or_else(|| IoError::Invalid("empty ASCII grid".into()))?;
        let width = first_line.split_whitespace().count();
        if width < 2 || !values.len().is_multiple_of(width) {
            return Err(IoError::Invalid(
                "cannot infer a rectangular ASCII grid".into(),
            ));
        }
        (width, values.len() / width)
    };

    let cellsize = header.get("cellsize").copied().unwrap_or(1.0);
    if !cellsize.is_finite() || cellsize <= 0.0 {
        return Err(IoError::Invalid(
            "ASCII grid cellsize must be positive".into(),
        ));
    }
    let origin_x = header
        .get("xllcorner")
        .or_else(|| header.get("xllcenter"))
        .copied()
        .unwrap_or(0.0);
    let origin_y = header
        .get("yllcorner")
        .or_else(|| header.get("yllcenter"))
        .copied()
        .unwrap_or(0.0);
    let nodata = header.get("nodata_value").copied();

    let mut heights = vec![0.0f32; width * height];
    let mut mask = nodata.map(|_| vec![true; width * height]);
    let mut valid_sum = 0.0f64;
    let mut valid_count = 0usize;
    for y in 0..height {
        // ESRI row 0 is north; VistaForge row 0 is south.
        let src_row = height - 1 - y;
        for x in 0..width {
            let raw = values[src_row * width + x];
            if !raw.is_finite() {
                return Err(IoError::Invalid(format!(
                    "non-finite sample at row {src_row}, column {x}"
                )));
            }
            let idx = y * width + x;
            if let Some(nd) = nodata
                && (raw - nd).abs() <= f64::EPSILON * nd.abs().max(1.0)
            {
                if let Some(mask) = &mut mask {
                    mask[idx] = false;
                }
                continue;
            }
            heights[idx] = raw as f32;
            valid_sum += raw;
            valid_count += 1;
        }
    }

    // Masked samples must hold finite values; use the valid mean as a neutral
    // floor so normals and bounds remain sane.
    if let Some(mask) = &mask {
        let fill = if valid_count > 0 {
            (valid_sum / valid_count as f64) as f32
        } else {
            0.0
        };
        for (i, valid) in mask.iter().enumerate() {
            if !valid {
                heights[i] = fill;
            }
        }
    }

    let mut hf = HeightField::new(width, height, cellsize, cellsize, heights)?;
    hf.set_origin(origin_x, origin_y);
    hf.set_nodata(nodata.map(|v| v as f32));
    if let Some(mask) = mask {
        hf.set_mask(Some(mask))?;
    }
    Ok(hf)
}

/// Read an ASCII grid file.
pub fn read_ascii_grid(path: &Path) -> Result<HeightField> {
    let text = std::fs::read_to_string(path)?;
    parse_ascii_grid(&text)
}

/// Write a heightfield as an ESRI ASCII Grid. Rows are written north first.
pub fn write_ascii_grid(path: &Path, hf: &HeightField, nodata: f32) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut out = std::io::BufWriter::new(file);
    let (sx, sy) = hf.spacing();
    let spacing = (sx + sy) * 0.5;
    let (ox, oy) = hf.origin();
    writeln!(out, "ncols {}", hf.width())?;
    writeln!(out, "nrows {}", hf.height())?;
    writeln!(out, "xllcorner {ox}")?;
    writeln!(out, "yllcorner {oy}")?;
    writeln!(out, "cellsize {spacing}")?;
    writeln!(out, "NODATA_value {nodata}")?;
    for y in (0..hf.height()).rev() {
        for x in 0..hf.width() {
            if x > 0 {
                write!(out, " ")?;
            }
            if hf.is_valid(x, y) {
                write!(out, "{:.3}", hf.get(x, y))?;
            } else {
                write!(out, "{nodata}")?;
            }
        }
        writeln!(out)?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esri_round_trip_reverses_rows() {
        // 2x2 north-first: row 0 = 10 20, row 1 = 30 40.
        let text = "\
ncols 2
nrows 2
xllcorner 100
yllcorner 200
cellsize 5
NODATA_value -9999
10 20
30 40
";
        let hf = parse_ascii_grid(text).unwrap();
        assert_eq!(hf.width(), 2);
        // South row first: 30 40, then 10 20.
        assert_eq!(hf.heights(), &[30.0, 40.0, 10.0, 20.0]);
        assert_eq!(hf.origin(), (100.0, 200.0));
        assert_eq!(hf.spacing(), (5.0, 5.0));
    }

    #[test]
    fn nodata_becomes_masked() {
        let text = "\
ncols 3
nrows 2
NODATA_value -9999
1 2 3
4 -9999 6
";
        let hf = parse_ascii_grid(text).unwrap();
        assert_eq!(hf.valid_count(), 5);
        // Row 0 (south) comes from the second input row.
        assert!(!hf.is_valid(1, 0));
    }

    #[test]
    fn matrix_mode_infers_shape() {
        let hf = parse_ascii_grid("1 2\n3 4\n5 6\n").unwrap();
        assert_eq!((hf.width(), hf.height()), (2, 3));
        assert_eq!(hf.get(0, 0), 5.0);
        assert_eq!(hf.get(1, 2), 2.0);
    }

    #[test]
    fn write_then_read() {
        let hf = HeightField::new(3, 2, 10.0, 10.0, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
        let dir = std::env::temp_dir().join(format!("vf-ascii-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("grid.asc");
        write_ascii_grid(&path, &hf, -9999.0).unwrap();
        let loaded = read_ascii_grid(&path).unwrap();
        assert_eq!(loaded.heights(), hf.heights());
        std::fs::remove_dir_all(&dir).ok();
    }
}
