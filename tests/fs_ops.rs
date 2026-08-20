use std::path::Path;

use albumfs::fs::format::format;
use albumfs::fs::{AlbumFs, FsError, UB};
use image::{Rgba, RgbaImage};

fn make_pool(dir: &Path, count: usize, size: u32) {
    for image_index in 0..count {
        let mut image = RgbaImage::new(size, size);
        for (pixel_index, pixel) in image.pixels_mut().enumerate() {
            let value = (pixel_index as u32)
                .wrapping_mul(37)
                .wrapping_add(image_index as u32 * 53);
            *pixel = Rgba([value as u8, (value >> 7) as u8, (value >> 15) as u8, 255]);
        }
        image
            .save(dir.join(format!("carrier-{image_index:03}.png")))
            .unwrap();
    }
}

fn formatted_fs(count: usize, size: u32) -> (tempfile::TempDir, AlbumFs) {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), count, size);
    format(dir.path()).unwrap();
    let fs = AlbumFs::open(dir.path()).unwrap();
    (dir, fs)
}

#[test]
fn create_read_write_small() {
    let (_dir, mut fs) = formatted_fs(2, 512);
    let ino = fs.create(1, "hello.txt", 0o644).unwrap();
    assert_eq!(fs.write(ino, 0, b"hello albumfs").unwrap(), 13);
    assert_eq!(fs.read(ino, 0, 100).unwrap(), b"hello albumfs");
    assert_eq!(fs.getattr(ino).unwrap().size, 13);
}

#[test]
fn write_spanning_many_blocks() {
    let (_dir, mut fs) = formatted_fs(4, 1024);
    let ino = fs.create(1, "many.bin", 0o644).unwrap();
    let payload: Vec<u8> = (0..(15 * UB + 137))
        .map(|index| (index.wrapping_mul(29) % 251) as u8)
        .collect();
    fs.write(ino, 0, &payload).unwrap();
    assert_eq!(fs.read(ino, 0, payload.len() + 10).unwrap(), payload);
    assert_eq!(fs.getattr(ino).unwrap().block_count, 16);
}

#[test]
fn double_indirect_reach() {
    let (_dir, mut fs) = formatted_fs(3, 1600);
    let ino = fs.create(1, "large.bin", 0o644).unwrap();
    let block_count = 12 + UB / 8 + 1;
    let payload: Vec<u8> = (0..(block_count * UB))
        .map(|index| (index.wrapping_mul(17) % 253) as u8)
        .collect();
    fs.write(ino, 0, &payload).unwrap();
    let inode = fs.getattr(ino).unwrap();
    assert_ne!(inode.double_indirect, 0);
    assert_eq!(inode.block_count as usize, block_count);
    assert_eq!(fs.read(ino, 0, payload.len()).unwrap(), payload);
}

#[test]
fn mkdir_and_readdir() {
    let (_dir, mut fs) = formatted_fs(2, 512);
    let photos = fs.mkdir(1, "photos", 0o755).unwrap();
    let keys = fs.mkdir(1, "keys", 0o755).unwrap();
    let notes = fs.create(1, "notes.txt", 0o644).unwrap();
    let entries = fs.readdir(1).unwrap();
    assert!(entries.contains(&(photos, "photos".into(), 2)));
    assert!(entries.contains(&(keys, "keys".into(), 2)));
    assert!(entries.contains(&(notes, "notes.txt".into(), 1)));
    assert!(entries.contains(&(1, ".".into(), 2)));
    assert!(entries.contains(&(1, "..".into(), 2)));
}

#[test]
fn unlink_frees_space() {
    let (_dir, mut fs) = formatted_fs(2, 768);
    let baseline = fs.free_blocks();
    let ino = fs.create(1, "temporary.bin", 0o644).unwrap();
    fs.write(ino, 0, &vec![0x93; 4 * UB + 7]).unwrap();
    assert_eq!(fs.free_blocks(), baseline - 5);
    fs.unlink(1, "temporary.bin").unwrap();
    assert_eq!(fs.free_blocks(), baseline);
    assert_eq!(fs.lookup(1, "temporary.bin").unwrap(), None);
}

#[test]
fn rmdir_rejects_nonempty() {
    let (_dir, mut fs) = formatted_fs(2, 512);
    let dir = fs.mkdir(1, "work", 0o755).unwrap();
    fs.create(dir, "item", 0o644).unwrap();
    assert!(matches!(fs.rmdir(1, "work"), Err(FsError::NotEmpty)));
    fs.unlink(dir, "item").unwrap();
    fs.rmdir(1, "work").unwrap();
    assert_eq!(fs.lookup(1, "work").unwrap(), None);
}

#[test]
fn rename_moves_and_replaces_entries() {
    let (_dir, mut fs) = formatted_fs(2, 512);
    let left = fs.mkdir(1, "left", 0o755).unwrap();
    let right = fs.mkdir(1, "right", 0o755).unwrap();
    let source = fs.create(left, "source.txt", 0o644).unwrap();
    fs.write(source, 0, b"rename payload").unwrap();
    let target = fs.create(right, "target.txt", 0o644).unwrap();
    fs.write(target, 0, b"old target").unwrap();

    fs.rename(left, "source.txt", right, "target.txt").unwrap();
    assert_eq!(fs.lookup(left, "source.txt").unwrap(), None);
    assert_eq!(fs.lookup(right, "target.txt").unwrap(), Some(source));
    assert_eq!(fs.read(source, 0, 100).unwrap(), b"rename payload");
    assert!(matches!(fs.getattr(target), Err(FsError::NotFound)));
}
