use std::path::{Path, PathBuf};

use albumfs::fs::format::{format, format_with_options, load_bitmap, open};
use albumfs::fs::superblock::{CarrierRange, Superblock};
use albumfs::fs::{FsError, FS_VERSION, PB, SB_MAGIC, UB};
use image::{Rgba, RgbaImage};
use rand::Rng;

fn make_pool(dir: &Path, count: usize, size: u32) -> Vec<(PathBuf, RgbaImage)> {
    let mut originals = Vec::new();
    let mut rng = rand::thread_rng();
    for index in 0..count {
        let mut image = RgbaImage::new(size, size);
        for pixel in image.pixels_mut() {
            *pixel = Rgba([rng.gen(), rng.gen(), rng.gen(), rng.gen()]);
        }
        let path = dir.join(format!("carrier-{index:03}.png"));
        image.save(&path).unwrap();
        originals.push((path, image));
    }
    originals
}

#[test]
fn format_then_open_roundtrips_geometry() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 3, 512);
    let formatted = format(dir.path()).unwrap();
    let (opened, _) = open(dir.path()).unwrap();

    assert_eq!(opened, formatted);
    assert_eq!(&opened.encode()[..8], &SB_MAGIC);
    assert_eq!(opened.version, FS_VERSION);
    assert_eq!(opened.block_size, PB as u32);
    assert_eq!(opened.usable_size, UB as u32);
    assert_eq!(opened.manifest.len(), 3);
    let mut next = 0;
    for carrier in &opened.manifest {
        assert_eq!(carrier.block_start, next);
        next += carrier.block_count;
    }
    assert_eq!(next, opened.total_blocks);
}

#[test]
fn free_count_after_format() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 2, 512);
    format(dir.path()).unwrap();
    let (sb, mut store) = open(dir.path()).unwrap();
    let bitmap = load_bitmap(&mut store, &sb).unwrap();
    assert_eq!(bitmap.count_free(), sb.total_blocks - sb.data_start - 1);
}

#[test]
fn carriers_still_open_as_images() {
    let dir = tempfile::tempdir().unwrap();
    let originals = make_pool(dir.path(), 3, 512);
    format(dir.path()).unwrap();

    for (path, before) in originals {
        let after = image::open(path).unwrap().to_rgba8();
        assert_eq!(before.dimensions(), after.dimensions());
        for (old, new) in before.pixels().zip(after.pixels()) {
            for channel in 0..3 {
                assert_eq!(old.0[channel] >> 1, new.0[channel] >> 1);
            }
            assert_eq!(old.0[3], new.0[3]);
        }
    }
}

#[test]
fn version_one_superblock_still_decodes() {
    let mut legacy = Superblock::standard(
        12,
        64,
        1,
        2,
        6,
        vec![CarrierRange {
            name: String::new(),
            chunk_index: 0,
            block_start: 0,
            block_count: 12,
        }],
        false,
    );
    legacy.version = 1;
    legacy.argon2_mem_kib = 0;
    legacy.argon2_iters = 0;
    legacy.argon2_parallel = 0;
    let decoded = Superblock::decode(&legacy.encode()).unwrap();
    assert_eq!(decoded, legacy);
    assert!(!decoded.encrypted());
}

#[test]
fn format_guard() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 2, 512);
    format(dir.path()).unwrap();

    let error = format(dir.path()).unwrap_err();
    assert!(matches!(error, FsError::AlreadyFormatted));
    format_with_options(dir.path(), None, true).unwrap();
    open(dir.path()).unwrap();
}
