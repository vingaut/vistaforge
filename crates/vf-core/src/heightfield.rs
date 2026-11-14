use std::fmt;

use crate::stats::TerrainStats;

/// Axis-aligned bounds in world metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    pub fn size(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    pub fn center(&self) -> [f64; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    pub fn extent(&self) -> f64 {
        let s = self.size();
        s[0].max(s[1]).max(s[2])
    }
}

/// A regular 2.5D elevation grid.
///
/// Heights are in metres, row-major from south (row 0) to north. The optional
/// validity mask marks samples that must not contribute to interpolation.
#[derive(Debug, Clone)]
pub struct HeightField {
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin: [f64; 2],
    heights: Vec<f32>,
    nodata: Option<f32>,
    mask: Option<Vec<bool>>,
}

impl HeightField {
    pub fn new(
        width: usize,
        height: usize,
        spacing_x: f64,
        spacing_y: f64,
        heights: Vec<f32>,
    ) -> Result<Self, HeightFieldError> {
        if width < 2 || height < 2 {
            return Err(HeightFieldError::TooSmall { width, height });
        }
        if heights.len() != width * height {
            return Err(HeightFieldError::WrongSampleCount {
                expected: width * height,
                actual: heights.len(),
            });
        }
        if !spacing_x.is_finite() || !spacing_y.is_finite() || spacing_x <= 0.0 || spacing_y <= 0.0
        {
            return Err(HeightFieldError::InvalidSpacing);
        }
        if let Some(pos) = heights.iter().position(|v| !v.is_finite()) {
            return Err(HeightFieldError::InvalidSample {
                index: pos,
                value: heights[pos],
            });
        }
        Ok(Self {
            width,
            height,
            spacing_x,
            spacing_y,
            origin: [0.0, 0.0],
            heights,
            nodata: None,
            mask: None,
        })
    }

    pub fn filled(
        width: usize,
        height: usize,
        spacing: f64,
        value: f32,
    ) -> Result<Self, HeightFieldError> {
        Self::new(width, height, spacing, spacing, vec![value; width * height])
    }

    pub fn from_i16(
        width: usize,
        height: usize,
        spacing_x: f64,
        spacing_y: f64,
        samples: &[i16],
    ) -> Result<Self, HeightFieldError> {
        if samples.len() != width * height {
            return Err(HeightFieldError::WrongSampleCount {
                expected: width * height,
                actual: samples.len(),
            });
        }
        let heights = samples.iter().map(|&v| f32::from(v)).collect();
        Self::new(width, height, spacing_x, spacing_y, heights)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn spacing(&self) -> (f64, f64) {
        (self.spacing_x, self.spacing_y)
    }

    pub fn origin(&self) -> (f64, f64) {
        (self.origin[0], self.origin[1])
    }

    /// Override the horizontal sample spacing (used when an importer knows
    /// the physical cell size and the file header does not).
    pub fn set_spacing(&mut self, spacing_x: f64, spacing_y: f64) -> Result<(), HeightFieldError> {
        if !spacing_x.is_finite() || !spacing_y.is_finite() || spacing_x <= 0.0 || spacing_y <= 0.0
        {
            return Err(HeightFieldError::InvalidSpacing);
        }
        self.spacing_x = spacing_x;
        self.spacing_y = spacing_y;
        Ok(())
    }

    pub fn set_origin(&mut self, x: f64, y: f64) {
        self.origin = [x, y];
    }

    pub fn sample_count(&self) -> usize {
        self.heights.len()
    }

    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    pub fn heights_mut(&mut self) -> &mut [f32] {
        &mut self.heights
    }

    pub fn nodata(&self) -> Option<f32> {
        self.nodata
    }

    pub fn set_nodata(&mut self, value: Option<f32>) {
        self.nodata = value;
    }

    pub fn mask(&self) -> Option<&[bool]> {
        self.mask.as_deref()
    }

    pub fn set_mask(&mut self, mask: Option<Vec<bool>>) -> Result<(), HeightFieldError> {
        if let Some(mask) = &mask
            && mask.len() != self.heights.len()
        {
            return Err(HeightFieldError::WrongMaskCount {
                expected: self.heights.len(),
                actual: mask.len(),
            });
        }
        self.mask = mask;
        Ok(())
    }

    pub fn clear_mask(&mut self) {
        self.mask = None;
    }

    #[inline]
    pub fn is_valid(&self, x: usize, y: usize) -> bool {
        match &self.mask {
            Some(mask) => mask[self.idx(x, y)],
            None => true,
        }
    }

    pub fn valid_count(&self) -> usize {
        match &self.mask {
            Some(mask) => mask.iter().filter(|&&v| v).count(),
            None => self.heights.len(),
        }
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize) -> usize {
        debug_assert!(x < self.width && y < self.height);
        y * self.width + x
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.heights[self.idx(x, y)]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: f32) {
        let i = self.idx(x, y);
        self.heights[i] = z;
    }

    pub fn min_max(&self) -> (f32, f32) {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for &v in &self.heights {
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
        }
        (min, max)
    }

    pub fn mean(&self) -> f32 {
        if self.heights.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.heights.iter().map(|&v| f64::from(v)).sum();
        (sum / self.heights.len() as f64) as f32
    }

    pub fn stats(&self) -> TerrainStats {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut valid = 0usize;
        for (i, &v) in self.heights.iter().enumerate() {
            if !self.mask.as_ref().is_none_or(|m| m[i]) {
                continue;
            }
            min = min.min(v);
            max = max.max(v);
            sum += f64::from(v);
            sum_sq += f64::from(v) * f64::from(v);
            valid += 1;
        }
        if valid == 0 {
            min = 0.0;
            max = 0.0;
        }
        let mean = if valid == 0 { 0.0 } else { sum / valid as f64 };
        let variance = if valid == 0 {
            0.0
        } else {
            (sum_sq / valid as f64 - mean * mean).max(0.0)
        };
        TerrainStats {
            width: self.width,
            height: self.height,
            valid_cells: valid,
            nodata_cells: self.heights.len() - valid,
            min,
            max,
            mean: mean as f32,
            std_dev: variance.sqrt() as f32,
        }
    }

    /// Bilinear sample in grid coordinates. Coordinates are clamped to the
    /// grid. Masked samples are skipped and the remaining weights renormalised.
    pub fn sample_bilinear(&self, x: f64, y: f64) -> f32 {
        let x = x.clamp(0.0, (self.width - 1) as f64);
        let y = y.clamp(0.0, (self.height - 1) as f64);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = (x - x0 as f64) as f32;
        let ty = (y - y0 as f64) as f32;

        let samples = [
            (x0, y0, (1.0 - tx) * (1.0 - ty)),
            (x1, y0, tx * (1.0 - ty)),
            (x0, y1, (1.0 - tx) * ty),
            (x1, y1, tx * ty),
        ];

        if self.mask.is_none() {
            let z00 = self.get(x0, y0);
            let z10 = self.get(x1, y0);
            let z01 = self.get(x0, y1);
            let z11 = self.get(x1, y1);
            let a = z00 + (z10 - z00) * tx;
            let b = z01 + (z11 - z01) * tx;
            return a + (b - a) * ty;
        }

        let mut acc = 0.0f32;
        let mut weight = 0.0f32;
        for (sx, sy, w) in samples {
            if self.is_valid(sx, sy) {
                acc += self.get(sx, sy) * w;
                weight += w;
            }
        }
        if weight > 1e-6 {
            acc / weight
        } else {
            self.nodata.unwrap_or(0.0)
        }
    }

    pub fn sample_nearest(&self, x: f64, y: f64) -> f32 {
        let x = x.round().clamp(0.0, (self.width - 1) as f64) as usize;
        let y = y.round().clamp(0.0, (self.height - 1) as f64) as usize;
        self.get(x, y)
    }

    /// Central-difference gradient in metres per metre.
    pub fn gradient_at(&self, x: usize, y: usize) -> (f32, f32) {
        let xm = x.saturating_sub(1);
        let xp = (x + 1).min(self.width - 1);
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(self.height - 1);
        let dx = (self.get(xp, y) - self.get(xm, y)) / ((xp - xm) as f32 * self.spacing_x as f32);
        let dy = (self.get(x, yp) - self.get(x, ym)) / ((yp - ym) as f32 * self.spacing_y as f32);
        (dx, dy)
    }

    /// World-space normal from central differences. `vertical_scale` is an
    /// artistic exaggeration (1.0 keeps the DEM's true proportions).
    pub fn normal_at(&self, x: usize, y: usize, vertical_scale: f32) -> [f32; 3] {
        let (dx, dy) = self.gradient_at(x, y);
        let nx = -dx * vertical_scale;
        let ny = -dy * vertical_scale;
        let nz = 1.0f32;
        let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-12);
        [nx / len, ny / len, nz / len]
    }

    pub fn bounds(&self) -> Bounds {
        let (min_z, max_z) = self.min_max();
        Bounds {
            min: [self.origin[0], self.origin[1], f64::from(min_z)],
            max: [
                self.origin[0] + (self.width - 1) as f64 * self.spacing_x,
                self.origin[1] + (self.height - 1) as f64 * self.spacing_y,
                f64::from(max_z),
            ],
        }
    }

    /// Copy a rectangular region. `x`/`y` are in samples, `w`/`h` in samples.
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Result<Self, HeightFieldError> {
        if w < 2 || h < 2 || x + w > self.width || y + h > self.height {
            return Err(HeightFieldError::InvalidCrop {
                x,
                y,
                width: w,
                height: h,
                field_width: self.width,
                field_height: self.height,
            });
        }
        let mut heights = Vec::with_capacity(w * h);
        let mut mask = self.mask.as_ref().map(|_| Vec::with_capacity(w * h));
        for row in y..y + h {
            let start = row * self.width + x;
            heights.extend_from_slice(&self.heights[start..start + w]);
            if let (Some(src), Some(dst)) = (&self.mask, &mut mask) {
                dst.extend_from_slice(&src[start..start + w]);
            }
        }
        let mut out = Self::new(w, h, self.spacing_x, self.spacing_y, heights)?;
        out.origin = [
            self.origin[0] + x as f64 * self.spacing_x,
            self.origin[1] + y as f64 * self.spacing_y,
        ];
        out.nodata = self.nodata;
        out.mask = mask;
        Ok(out)
    }

    /// Apply `f` to every valid sample, leaving masked samples untouched.
    pub fn map_valid<F: FnMut(f32) -> f32>(&mut self, mut f: F) {
        for i in 0..self.heights.len() {
            if self.mask.as_ref().is_none_or(|m| m[i]) {
                self.heights[i] = f(self.heights[i]);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HeightFieldError {
    TooSmall {
        width: usize,
        height: usize,
    },
    WrongSampleCount {
        expected: usize,
        actual: usize,
    },
    WrongMaskCount {
        expected: usize,
        actual: usize,
    },
    InvalidSpacing,
    InvalidSample {
        index: usize,
        value: f32,
    },
    InvalidCrop {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        field_width: usize,
        field_height: usize,
    },
}

impl fmt::Display for HeightFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall { width, height } => {
                write!(f, "heightfield too small: {width}x{height}")
            }
            Self::WrongSampleCount { expected, actual } => {
                write!(f, "wrong sample count: expected {expected}, got {actual}")
            }
            Self::WrongMaskCount { expected, actual } => {
                write!(f, "wrong mask count: expected {expected}, got {actual}")
            }
            Self::InvalidSpacing => write!(f, "sample spacing must be finite and positive"),
            Self::InvalidSample { index, value } => {
                write!(f, "invalid height at index {index}: {value}")
            }
            Self::InvalidCrop {
                x,
                y,
                width,
                height,
                field_width,
                field_height,
            } => write!(
                f,
                "invalid crop {width}x{height}+{x}+{y} for {field_width}x{field_height} field"
            ),
        }
    }
}

impl std::error::Error for HeightFieldError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_midpoint() {
        let hf = HeightField::new(2, 2, 1.0, 1.0, vec![0.0, 10.0, 20.0, 30.0]).unwrap();
        assert!((hf.sample_bilinear(0.5, 0.5) - 15.0).abs() < 1e-6);
        assert!((hf.sample_bilinear(-10.0, 99.0) - 20.0).abs() < 1e-6);
    }

    #[test]
    fn normals_are_unit_length() {
        let hf = HeightField::new(3, 3, 1.0, 1.0, vec![0.0; 9]).unwrap();
        let n = hf.normal_at(1, 1, 1.0);
        assert!((n[0] * n[0] + n[1] * n[1] + n[2] * n[2] - 1.0).abs() < 1e-6);
        let slope = HeightField::new(
            3,
            3,
            1.0,
            1.0,
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .unwrap();
        assert_eq!(slope.normal_at(1, 1, 1.0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn stats_and_mask() {
        let mut hf = HeightField::new(2, 2, 1.0, 1.0, vec![0.0, 1.0, 2.0, 3.0]).unwrap();
        hf.set_mask(Some(vec![true, true, true, false])).unwrap();
        let s = hf.stats();
        assert_eq!(s.valid_cells, 3);
        assert_eq!(s.nodata_cells, 1);
        assert!((s.mean - 1.0).abs() < 1e-6);
    }

    #[test]
    fn crop_keeps_origin() {
        let hf = HeightField::new(3, 3, 2.0, 2.0, (0..9).map(|v| v as f32).collect()).unwrap();
        let c = hf.crop(1, 1, 2, 2).unwrap();
        assert_eq!((c.width(), c.height()), (2, 2));
        assert_eq!(c.origin(), (2.0, 2.0));
        assert_eq!(c.heights(), &[4.0, 5.0, 7.0, 8.0]);
        assert!(hf.crop(2, 2, 2, 2).is_err());
    }
}
