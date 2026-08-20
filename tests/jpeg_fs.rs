use std::collections::BTreeSet;
use std::path::Path;

use albumfs::fs::format::{format, format_with_anchor_options};
use albumfs::fs::{AlbumFs, UB};
use image::{DynamicImage, Rgba, RgbaImage};

fn textured_image(width: u32, height: u32, seed: u32) -> RgbaImage {
    let mut image = RgbaImage::new(width, height);
    let mut state = seed;
    for (index, pixel) in image.pixels_mut().enumerate() {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let x = index as u32 % width;
        let y = index as u32 / width;
        let noise = (state >> 24) as u8;
        *pixel = Rgba([
            (x as u8).wrapping_add(noise / 2),
            (y as u8).wrapping_add(noise / 3),
            ((x * 3 + y * 5) as u8).wrapping_add(noise / 4),
            255,
        ]);
    }
    image
}

fn make_jpeg_pool(dir: &Path, count: usize) {
    for index in 0..count {
        DynamicImage::ImageRgba8(textured_image(512, 512, index as u32 + 10))
            .save(dir.join(format!("carrier-{index:03}.jpg")))
            .unwrap();
    }
}

fn names(fs: &mut AlbumFs, ino: u64) -> BTreeSet<String> {
    fs.readdir(ino)
        .unwrap()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect()
}

#[test]
fn fs_over_jpeg_reload() {
    let dir = tempfile::tempdir().unwrap();
    make_jpeg_pool(dir.path(), 8);
    format(dir.path()).unwrap();
    let mut fs = AlbumFs::open(dir.path()).unwrap();
    let folder = fs.mkdir(1, "records", 0o755).unwrap();
    let file = fs.create(folder, "ledger.bin", 0o644).unwrap();
    let payload: Vec<u8> = (0..(2 * UB + 37))
        .map(|index| (index * 19 % 251) as u8)
        .collect();
    fs.write(file, 0, &payload).unwrap();
    fs.sync().unwrap();
    drop(fs);

    let mut reopened = AlbumFs::open(dir.path()).unwrap();
    let folder = reopened.lookup(1, "records").unwrap().unwrap();
    let file = reopened.lookup(folder, "ledger.bin").unwrap().unwrap();
    assert_eq!(reopened.read(file, 0, payload.len()).unwrap(), payload);
    assert_eq!(
        names(&mut reopened, 1),
        [".", "..", "records"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert_eq!(
        names(&mut reopened, folder),
        [".", "..", "ledger.bin"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn mixed_pool_reload() {
    let dir = tempfile::tempdir().unwrap();
    make_jpeg_pool(dir.path(), 4);
    for index in 0..2 {
        textured_image(512, 512, index as u32 + 100)
            .save(dir.path().join(format!("carrier-png-{index:03}.png")))
            .unwrap();
    }
    let sb = format(dir.path()).unwrap();
    assert_eq!(sb.manifest.len(), 6);
    let mut fs = AlbumFs::open(dir.path()).unwrap();
    let file = fs.create(1, "mixed.dat", 0o644).unwrap();
    let payload: Vec<u8> = (0..(UB + 91))
        .map(|index| (index * 23 % 253) as u8)
        .collect();
    fs.write(file, 0, &payload).unwrap();
    fs.sync().unwrap();
    drop(fs);

    let mut reopened = AlbumFs::open(dir.path()).unwrap();
    let file = reopened.lookup(1, "mixed.dat").unwrap().unwrap();
    assert_eq!(reopened.read(file, 0, payload.len()).unwrap(), payload);
}

#[test]
fn encrypted_jpeg_pool_reload() {
    let dir = tempfile::tempdir().unwrap();
    make_jpeg_pool(dir.path(), 10);
    let anchor = dir.path().join("carrier-000.jpg");
    let result = format_with_anchor_options(
        dir.path(),
        Some("jpeg markerless passphrase"),
        Some(&anchor),
        false,
    )
    .unwrap();
    assert!(result.superblock.markerless_encrypted());
    assert_eq!(result.superblock.manifest.len(), 9);

    let mut fs =
        AlbumFs::open_with_anchor(dir.path(), &anchor, "jpeg markerless passphrase").unwrap();
    let directory = fs.mkdir(1, "sealed", 0o700).unwrap();
    let file = fs.create(directory, "payload.bin", 0o600).unwrap();
    let payload: Vec<u8> = (0..(UB + 113))
        .map(|index| (index.wrapping_mul(37) % 251) as u8)
        .collect();
    fs.write(file, 0, &payload).unwrap();
    fs.sync().unwrap();
    drop(fs);

    let mut reopened =
        AlbumFs::open_with_anchor(dir.path(), &anchor, "jpeg markerless passphrase").unwrap();
    let directory = reopened.lookup(1, "sealed").unwrap().unwrap();
    let file = reopened.lookup(directory, "payload.bin").unwrap().unwrap();
    assert_eq!(reopened.read(file, 0, payload.len()).unwrap(), payload);
}
