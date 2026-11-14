//! Legacy DEM v1 reader/writer.
//!
//! Layout (all header integers are big-endian):
//!
//! ```text
//! 0x000  16  "Vista DEM File\0\0"
//! 0x010  16  reserved
//! 0x020  32  title (NUL padded)
//! 0x040  64  comment (NUL padded)
//! 0x080   4  version = 1
//! 0x084   4  flags (bit 0 = width/height valid)
//! 0x088   4  width  (0 => 258)
//! 0x08C   4  height (0 => 258)
//! 0x0C0 832  session blob (opaque)
//! 0x400 1024 256 x 4-byte palette entries
//! 0x800      rows: u16be payload_len + RLE(delta stream)
//! ```
//!
//! A row decodes to `width` i16 samples. The first sample is big-endian;
//! each following byte is a signed 8-bit delta, or `0x80` followed by a
//! big-endian absolute i16. The delta stream is RLE-compressed:
//! tokens 0x00..=0x7E are literals of `token + 1` bytes, tokens
//! 0x81..=0xFF repeat the next byte `257 - token` times.

use std::fmt;

use vf_core::HeightField;

pub const HEADER_SIZE: usize = 0x800;
pub const SIGNATURE: &[u8; 16] = b"Vista DEM File\0\0";
pub const DEFAULT_SIZE: usize = 258;
const PALETTE_OFFSET: usize = 0x400;
const SESSION_OFFSET: usize = 0x0C0;
const SESSION_LEN: usize = 0x400 - 0x0C0;

#[derive(Debug, Clone)]
pub struct DemHeader {
    /// The original 0x800 header bytes, preserved for a lossless session
    /// blob and unknown fields.
    pub raw: [u8; HEADER_SIZE],
    pub version: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub comment: String,
}

impl DemHeader {
    pub fn parse(raw: [u8; HEADER_SIZE]) -> Result<Self, DemError> {
        if &raw[0..16] != SIGNATURE {
            return Err(DemError::BadSignature);
        }
        let version = read_u32_be(&raw, 0x80);
        let flags = read_u32_be(&raw, 0x84);
        let width = read_u32_be(&raw, 0x88);
        let height = read_u32_be(&raw, 0x8C);
        let title = c_string(&raw[0x20..0x40]);
        let comment = c_string(&raw[0x40..0x80]);
        let header = Self {
            raw,
            version,
            flags,
            width,
            height,
            title,
            comment,
        };
        header.validate()?;
        Ok(header)
    }

    /// Build a new header. `raw` is zero-filled apart from the fields we
    /// know how to write.
    pub fn new(title: &str, comment: &str, width: u32, height: u32) -> Self {
        let mut raw = [0u8; HEADER_SIZE];
        raw[0..16].copy_from_slice(SIGNATURE);
        write_c_string(&mut raw[0x20..0x40], title);
        write_c_string(&mut raw[0x40..0x80], comment);
        let flags: u32 = if width == DEFAULT_SIZE as u32 && height == DEFAULT_SIZE as u32 {
            0
        } else {
            1
        };
        raw[0x80..0x84].copy_from_slice(&1u32.to_be_bytes());
        raw[0x84..0x88].copy_from_slice(&flags.to_be_bytes());
        raw[0x88..0x8C].copy_from_slice(&width.to_be_bytes());
        raw[0x8C..0x90].copy_from_slice(&height.to_be_bytes());
        raw[0x90..0x94].copy_from_slice(&1u32.to_be_bytes());
        Self {
            raw,
            version: 1,
            flags,
            width,
            height,
            title: title.to_owned(),
            comment: comment.to_owned(),
        }
    }

    fn validate(&self) -> Result<(), DemError> {
        if self.version != 1 {
            return Err(DemError::UnsupportedVersion(self.version));
        }
        let (w, h) = self.effective_dimensions();
        if w < 2 || h < 2 {
            return Err(DemError::InvalidDimensions {
                width: w,
                height: h,
            });
        }
        if w > 8192 || h > 8192 {
            return Err(DemError::InvalidDimensions {
                width: w,
                height: h,
            });
        }
        if w.checked_mul(h).is_none() {
            return Err(DemError::InvalidDimensions {
                width: w,
                height: h,
            });
        }
        Ok(())
    }

    /// The legacy format treats `flags & 1 == 0` as "use the 258x258 default",
    /// even when width/height contain another value. A zero dimension is
    /// also treated as the default.
    pub fn effective_dimensions(&self) -> (usize, usize) {
        if self.flags & 1 == 0 || self.width == 0 || self.height == 0 {
            (DEFAULT_SIZE, DEFAULT_SIZE)
        } else {
            (self.width as usize, self.height as usize)
        }
    }

    pub fn palette(&self) -> &[u8] {
        &self.raw[PALETTE_OFFSET..HEADER_SIZE]
    }

    pub fn session_blob(&self) -> &[u8] {
        &self.raw[SESSION_OFFSET..SESSION_OFFSET + SESSION_LEN]
    }
}

#[derive(Debug, Clone)]
pub struct DemFile {
    pub header: DemHeader,
    /// Row-major, south row first, `width * height` samples in metres.
    pub heights: Vec<i16>,
    /// 256 entries of four bytes; the exact RGB/BGR order is not settled yet.
    pub palette: Vec<[u8; 4]>,
    /// Opaque compatibility blob from 0x0C0..0x400.
    pub session: [u8; SESSION_LEN],
}

impl DemFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, DemError> {
        if bytes.len() < HEADER_SIZE {
            return Err(DemError::TruncatedHeader {
                needed: HEADER_SIZE,
                actual: bytes.len(),
            });
        }
        let mut raw = [0u8; HEADER_SIZE];
        raw.copy_from_slice(&bytes[..HEADER_SIZE]);
        let header = DemHeader::parse(raw)?;
        let (width, height) = header.effective_dimensions();

        let mut palette = Vec::with_capacity(256);
        for i in 0..256 {
            let off = PALETTE_OFFSET + i * 4;
            palette.push([
                header.raw[off],
                header.raw[off + 1],
                header.raw[off + 2],
                header.raw[off + 3],
            ]);
        }
        let mut session = [0u8; SESSION_LEN];
        session.copy_from_slice(&header.raw[SESSION_OFFSET..SESSION_OFFSET + SESSION_LEN]);

        let mut heights = Vec::with_capacity(width * height);
        let mut off = HEADER_SIZE;
        for row in 0..height {
            if off + 2 > bytes.len() {
                return Err(DemError::TruncatedRow {
                    row,
                    needed: 2,
                    actual: 0,
                });
            }
            let len = usize::from(u16::from_be_bytes([bytes[off], bytes[off + 1]]));
            off += 2;
            if off + len > bytes.len() {
                return Err(DemError::TruncatedRow {
                    row,
                    needed: len,
                    actual: bytes.len().saturating_sub(off),
                });
            }
            let payload = &bytes[off..off + len];
            off += len;
            let samples = decode_row(payload, width, row)?;
            heights.extend_from_slice(&samples);
        }
        if off != bytes.len() {
            return Err(DemError::TrailingData {
                expected: off,
                actual: bytes.len(),
            });
        }
        Ok(Self {
            header,
            heights,
            palette,
            session,
        })
    }

    pub fn write(&self) -> Result<Vec<u8>, DemError> {
        let (width, height) = self.header.effective_dimensions();
        if self.heights.len() != width * height {
            return Err(DemError::WrongSampleCount {
                expected: width * height,
                actual: self.heights.len(),
            });
        }

        let mut raw = self.header.raw;
        // Keep session bits, but make the dimension bit consistent with the
        // actual data so other readers do not default to 258x258.
        let dims_bit = if width == DEFAULT_SIZE && height == DEFAULT_SIZE {
            0
        } else {
            1
        };
        let flags = (self.header.flags & !1) | dims_bit;
        raw[0x84..0x88].copy_from_slice(&flags.to_be_bytes());
        raw[0x88..0x8C].copy_from_slice(&(width as u32).to_be_bytes());
        raw[0x8C..0x90].copy_from_slice(&(height as u32).to_be_bytes());
        write_c_string(&mut raw[0x20..0x40], &self.header.title);
        write_c_string(&mut raw[0x40..0x80], &self.header.comment);
        raw[SESSION_OFFSET..SESSION_OFFSET + SESSION_LEN].copy_from_slice(&self.session);
        for (i, entry) in self.palette.iter().enumerate().take(256) {
            let off = PALETTE_OFFSET + i * 4;
            raw[off..off + 4].copy_from_slice(entry);
        }

        let mut out = Vec::with_capacity(HEADER_SIZE + width * height);
        out.extend_from_slice(&raw);
        for row in self.heights.chunks_exact(width) {
            let payload = encode_row(row);
            if payload.len() > u16::MAX as usize {
                return Err(DemError::RowTooLong { len: payload.len() });
            }
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            out.extend_from_slice(&payload);
        }
        Ok(out)
    }

    pub fn width(&self) -> usize {
        self.header.effective_dimensions().0
    }

    pub fn height(&self) -> usize {
        self.header.effective_dimensions().1
    }

    pub fn height_at(&self, x: usize, y: usize) -> Option<i16> {
        if x >= self.width() || y >= self.height() {
            return None;
        }
        self.heights.get(y * self.width() + x).copied()
    }

    /// Convert to a VistaForge heightfield. DEM samples are metres; the
    /// caller supplies the horizontal spacing (usually 30 m).
    pub fn to_heightfield(&self, spacing: f32) -> Result<HeightField, vf_core::Error> {
        let (width, height) = self.header.effective_dimensions();
        let mut hf = HeightField::from_i16(
            width,
            height,
            f64::from(spacing),
            f64::from(spacing),
            &self.heights,
        )?;
        hf.set_origin(0.0, 0.0);
        Ok(hf)
    }

    /// Build a DEM from a heightfield. Values are rounded and clamped to i16
    /// because the legacy format stores integer metres.
    pub fn from_heightfield(
        hf: &HeightField,
        title: &str,
        comment: &str,
        palette: Vec<[u8; 4]>,
    ) -> Result<Self, vf_core::Error> {
        let mut heights = Vec::with_capacity(hf.sample_count());
        for &value in hf.heights() {
            heights.push(
                value
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            );
        }
        let header = DemHeader::new(title, comment, hf.width() as u32, hf.height() as u32);
        let mut palette = palette;
        palette.truncate(256);
        palette.resize(256, [0, 0, 0, 0]);
        Ok(Self {
            header,
            heights,
            palette,
            session: [0u8; SESSION_LEN],
        })
    }

    /// Palette as RGB triples. `bgr` selects the legacy COLORREF interpretation
    /// (bytes 1..3 = B,G,R) instead of R,G,B.
    pub fn palette_rgb(&self, bgr: bool) -> Vec<[u8; 3]> {
        self.palette
            .iter()
            .map(|entry| {
                if bgr {
                    [entry[3], entry[2], entry[1]]
                } else {
                    [entry[1], entry[2], entry[3]]
                }
            })
            .collect()
    }

    pub fn min_max_mean(&self) -> (i16, i16, f64) {
        let mut min = i16::MAX;
        let mut max = i16::MIN;
        let mut sum = 0i64;
        for &v in &self.heights {
            min = min.min(v);
            max = max.max(v);
            sum += i64::from(v);
        }
        let mean = if self.heights.is_empty() {
            0.0
        } else {
            sum as f64 / self.heights.len() as f64
        };
        (min, max, mean)
    }
}

fn decode_row(payload: &[u8], width: usize, row: usize) -> Result<Vec<i16>, DemError> {
    let delta = rle_decode(payload, row)?;
    if delta.len() < 2 {
        return Err(DemError::TruncatedDelta {
            row,
            needed: 2,
            actual: delta.len(),
        });
    }
    let mut out = Vec::with_capacity(width);
    let mut cur = i16::from_be_bytes([delta[0], delta[1]]);
    out.push(cur);
    let mut i = 2;
    while out.len() < width {
        if i >= delta.len() {
            return Err(DemError::TruncatedDelta {
                row,
                needed: width,
                actual: out.len(),
            });
        }
        let b = delta[i];
        i += 1;
        if b == 0x80 {
            if i + 2 > delta.len() {
                return Err(DemError::TruncatedDelta {
                    row,
                    needed: width,
                    actual: out.len(),
                });
            }
            cur = i16::from_be_bytes([delta[i], delta[i + 1]]);
            i += 2;
        } else {
            cur = cur.wrapping_add(i16::from(b as i8));
        }
        out.push(cur);
    }
    if i != delta.len() {
        return Err(DemError::TrailingDeltaBytes {
            row,
            consumed: i,
            total: delta.len(),
        });
    }
    Ok(out)
}

fn rle_decode(payload: &[u8], row: usize) -> Result<Vec<u8>, DemError> {
    let mut out = Vec::with_capacity(payload.len() * 2);
    let mut i = 0;
    while i < payload.len() {
        let token = payload[i];
        i += 1;
        match token {
            0x00..=0x7E => {
                let n = usize::from(token) + 1;
                if i + n > payload.len() {
                    return Err(DemError::TruncatedRle {
                        row,
                        offset: i,
                        needed: n,
                        actual: payload.len() - i,
                    });
                }
                out.extend_from_slice(&payload[i..i + n]);
                i += n;
            }
            0x81..=0xFF => {
                if i >= payload.len() {
                    return Err(DemError::TruncatedRle {
                        row,
                        offset: i,
                        needed: 1,
                        actual: 0,
                    });
                }
                let n = 257 - usize::from(token);
                let value = payload[i];
                i += 1;
                out.extend(std::iter::repeat_n(value, n));
            }
            _ => return Err(DemError::BadRleToken { row, token }),
        }
    }
    Ok(out)
}

fn encode_row(samples: &[i16]) -> Vec<u8> {
    debug_assert!(!samples.is_empty());
    let mut delta = Vec::with_capacity(samples.len() * 2);
    delta.extend_from_slice(&samples[0].to_be_bytes());
    for pair in samples.windows(2) {
        let d = i32::from(pair[1]) - i32::from(pair[0]);
        if (-127..=127).contains(&d) {
            delta.push(d as i8 as u8);
        } else {
            delta.push(0x80);
            delta.extend_from_slice(&pair[1].to_be_bytes());
        }
    }
    rle_encode(&delta)
}

fn rle_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let mut run = 1;
        while i + run < data.len() && data[i + run] == data[i] && run < 128 {
            run += 1;
        }
        if run >= 3 {
            out.push((257 - run) as u8);
            out.push(data[i]);
            i += run;
            continue;
        }

        let start = i;
        let mut end = i + 1;
        while end < data.len() && end - start < 127 {
            // Stop before a run of at least three identical bytes so it can
            // be encoded as a repeat token.
            if end + 2 < data.len() && data[end] == data[end + 1] && data[end] == data[end + 2] {
                break;
            }
            end += 1;
        }
        let n = end - start;
        out.push((n - 1) as u8);
        out.extend_from_slice(&data[start..end]);
        i = end;
    }
    out
}

fn read_u32_be(bytes: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

fn write_c_string(dst: &mut [u8], value: &str) {
    dst.fill(0);
    let bytes = value.as_bytes();
    let n = bytes.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&bytes[..n]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemError {
    TruncatedHeader {
        needed: usize,
        actual: usize,
    },
    BadSignature,
    UnsupportedVersion(u32),
    InvalidDimensions {
        width: usize,
        height: usize,
    },
    TruncatedRow {
        row: usize,
        needed: usize,
        actual: usize,
    },
    TruncatedRle {
        row: usize,
        offset: usize,
        needed: usize,
        actual: usize,
    },
    BadRleToken {
        row: usize,
        token: u8,
    },
    TruncatedDelta {
        row: usize,
        needed: usize,
        actual: usize,
    },
    TrailingDeltaBytes {
        row: usize,
        consumed: usize,
        total: usize,
    },
    TrailingData {
        expected: usize,
        actual: usize,
    },
    WrongSampleCount {
        expected: usize,
        actual: usize,
    },
    RowTooLong {
        len: usize,
    },
}

impl fmt::Display for DemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader { needed, actual } => {
                write!(f, "truncated DEM header: need {needed} bytes, got {actual}")
            }
            Self::BadSignature => write!(f, "not a Vista DEM file (bad signature)"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported DEM version {v}"),
            Self::InvalidDimensions { width, height } => {
                write!(f, "invalid DEM dimensions {width}x{height}")
            }
            Self::TruncatedRow {
                row,
                needed,
                actual,
            } => {
                write!(f, "truncated row {row}: need {needed} bytes, got {actual}")
            }
            Self::TruncatedRle {
                row,
                offset,
                needed,
                actual,
            } => write!(
                f,
                "truncated RLE in row {row} at offset {offset}: need {needed} bytes, got {actual}"
            ),
            Self::BadRleToken { row, token } => {
                write!(f, "invalid RLE token 0x{token:02X} in row {row}")
            }
            Self::TruncatedDelta {
                row,
                needed,
                actual,
            } => {
                write!(
                    f,
                    "truncated delta stream in row {row}: need {needed} samples, got {actual}"
                )
            }
            Self::TrailingDeltaBytes {
                row,
                consumed,
                total,
            } => write!(
                f,
                "trailing bytes in delta stream of row {row}: consumed {consumed}/{total}"
            ),
            Self::TrailingData { expected, actual } => {
                write!(f, "trailing data: expected {expected} bytes, got {actual}")
            }
            Self::WrongSampleCount { expected, actual } => {
                write!(f, "wrong sample count: expected {expected}, got {actual}")
            }
            Self::RowTooLong { len } => write!(f, "encoded row too long: {len} bytes"),
        }
    }
}

impl std::error::Error for DemError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_round_trip() {
        let cases: &[&[u8]] = &[
            &[0],
            &[1, 2, 3],
            &[7; 3],
            &[7; 128],
            &[0, 0, 0, 1, 2, 3, 3, 3],
            &[0x80, 0x80, 0x80],
        ];
        for case in cases {
            let encoded = rle_encode(case);
            let decoded = rle_decode(&encoded, 0).unwrap();
            assert_eq!(&decoded, case);
        }
    }

    #[test]
    fn row_round_trip_with_escapes() {
        let samples = [0i16, 300, -100, 127, 128, -128, 32767, -32768, 42];
        let encoded = encode_row(&samples);
        let decoded = decode_row(&encoded, samples.len(), 0).unwrap();
        assert_eq!(decoded, samples);
    }

    #[test]
    fn header_new_defaults() {
        let h = DemHeader::new("Test", "Comment", 258, 258);
        assert_eq!(h.effective_dimensions(), (258, 258));
        assert_eq!(h.flags, 0);
    }

    #[test]
    fn header_new_non_default() {
        let h = DemHeader::new("Test", "", 514, 514);
        assert_eq!(h.effective_dimensions(), (514, 514));
        assert_eq!(h.flags & 1, 1);
    }
}
