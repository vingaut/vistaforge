use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use vf_io::{DemError, DemFile, DemHeader};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/unpacked")
}

fn collect_dem(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dem(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "DE") {
            out.push(path);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[test]
fn parses_the_entire_legacy_corpus() {
    let dir = corpus_dir();
    if !dir.exists() {
        eprintln!("skipping: legacy corpus not present at {}", dir.display());
        return;
    }

    let mut files = Vec::new();
    collect_dem(&dir, &mut files);
    files.sort();
    assert!(
        files.len() >= 30,
        "expected at least 30 DEM files, found {}",
        files.len()
    );

    for path in &files {
        let bytes = std::fs::read(path).expect("read DEM");
        let dem = DemFile::parse(&bytes)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
        let (w, h) = dem.header.effective_dimensions();
        assert!(
            w >= 2 && h >= 2,
            "{}: bad dimensions {w}x{h}",
            path.display()
        );
        assert_eq!(
            dem.heights.len(),
            w * h,
            "{}: wrong sample count",
            path.display()
        );
    }
}

#[test]
fn alps_matches_known_vectors() {
    let path = corpus_dir().join("disk03/ALPS.DE");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let dem = DemFile::parse(&bytes).unwrap();

    assert_eq!(dem.header.title, "The Alps");
    assert_eq!(dem.header.version, 1);
    assert_eq!(dem.header.effective_dimensions(), (258, 258));
    assert_eq!(dem.heights.len(), 258 * 258);

    let (min, max, mean) = dem.min_max_mean();
    assert_eq!((min, max), (4, 2287));
    assert!((mean - 1_312.322_892_254_071_3).abs() < 1e-9);

    assert_eq!(&dem.heights[..8], &[408, 376, 346, 316, 286, 256, 226, 198]);
    assert_eq!(
        &dem.heights[dem.heights.len() - 8..],
        &[480, 440, 402, 398, 394, 390, 396, 404]
    );

    let mut sample_bytes = Vec::with_capacity(dem.heights.len() * 2);
    for v in &dem.heights {
        sample_bytes.extend_from_slice(&v.to_le_bytes());
    }
    let digest = Sha256::digest(&sample_bytes);
    assert_eq!(
        hex(&digest),
        "53c0323ba35aa71e26ca21c18975a9e55390c4ccff4bd7eb7abd004dc6756061"
    );
}

#[test]
fn roundtrip_preserves_samples_and_palette() {
    let path = corpus_dir().join("disk03/ALPS.DE");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let dem = DemFile::parse(&bytes).unwrap();
    let encoded = dem.write().unwrap();
    let reparsed = DemFile::parse(&encoded).unwrap();

    assert_eq!(dem.heights, reparsed.heights);
    assert_eq!(dem.palette, reparsed.palette);
    assert_eq!(dem.header.title, reparsed.header.title);
    assert_eq!(dem.header.comment, reparsed.header.comment);
    assert_eq!(
        dem.header.effective_dimensions(),
        reparsed.header.effective_dimensions()
    );
}

#[test]
fn writes_and_reads_a_514_grid() {
    let width = 514;
    let height = 514;
    let heights: Vec<i16> = (0..width * height)
        .map(|i| ((i % 1000) as i16) - 500)
        .collect();
    let dem = DemFile {
        header: DemHeader::new("Synthetic 514", "unit test", width as u32, height as u32),
        heights: heights.clone(),
        palette: vec![[0, 0, 0, 0]; 256],
        session: [0u8; 832],
    };
    let bytes = dem.write().unwrap();
    let parsed = DemFile::parse(&bytes).unwrap();
    assert_eq!(parsed.header.effective_dimensions(), (514, 514));
    assert_eq!(parsed.heights, heights);
    assert_eq!(parsed.header.flags & 1, 1);
}

#[test]
fn rejects_bad_signature_and_truncation() {
    let path = corpus_dir().join("disk03/ALPS.DE");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] = b'X';
    assert_eq!(DemFile::parse(&bytes).unwrap_err(), DemError::BadSignature);

    let bytes = std::fs::read(&path).unwrap();
    assert!(matches!(
        DemFile::parse(&bytes[..0x400]).unwrap_err(),
        DemError::TruncatedHeader { .. }
    ));
}
