use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use albumfs::fs::format::format;
use albumfs::fs::{AlbumFs, UB};
use image::{Rgba, RgbaImage};

fn make_pool(dir: &Path, count: usize, size: u32) -> Vec<(PathBuf, RgbaImage)> {
    let mut originals = Vec::new();
    for image_index in 0..count {
        let mut image = RgbaImage::new(size, size);
        for (pixel_index, pixel) in image.pixels_mut().enumerate() {
            let value = (pixel_index as u32)
                .wrapping_mul(71)
                .wrapping_add(image_index as u32 * 101);
            *pixel = Rgba([
                value as u8,
                (value >> 5) as u8,
                (value >> 13) as u8,
                200 + image_index as u8,
            ]);
        }
        let path = dir.join(format!("carrier-{image_index:03}.png"));
        image.save(&path).unwrap();
        originals.push((path, image));
    }
    originals
}

fn build_tree(fs: &mut AlbumFs) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let notes: Vec<u8> = (0..40).map(|index| b'A' + index % 26).collect();
    let list: Vec<u8> = (0..(3 * UB + 17))
        .map(|index| (index.wrapping_mul(43) % 251) as u8)
        .collect();
    let key: Vec<u8> = (0usize..512)
        .map(|index| (index.wrapping_mul(11) % 256) as u8)
        .collect();

    let notes_ino = fs.create(1, "notes.txt", 0o644).unwrap();
    fs.write(notes_ino, 0, &notes).unwrap();
    let photos = fs.mkdir(1, "photos", 0o755).unwrap();
    let list_ino = fs.create(photos, "list.csv", 0o644).unwrap();
    fs.write(list_ino, 0, &list).unwrap();
    let keys = fs.mkdir(1, "keys", 0o755).unwrap();
    let key_ino = fs.create(keys, "id_ed25519", 0o600).unwrap();
    fs.write(key_ino, 0, &key).unwrap();
    (notes, list, key)
}

fn names(fs: &mut AlbumFs, ino: u64) -> BTreeSet<String> {
    fs.readdir(ino)
        .unwrap()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect()
}

#[test]
fn tree_survives_full_reload() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 3, 768);
    format(dir.path()).unwrap();
    let mut fs = AlbumFs::open(dir.path()).unwrap();
    let (notes, list, key) = build_tree(&mut fs);
    fs.sync().unwrap();
    drop(fs);

    let mut fs = AlbumFs::open(dir.path()).unwrap();
    let notes_ino = fs.lookup(1, "notes.txt").unwrap().unwrap();
    let photos = fs.lookup(1, "photos").unwrap().unwrap();
    let list_ino = fs.lookup(photos, "list.csv").unwrap().unwrap();
    let keys = fs.lookup(1, "keys").unwrap().unwrap();
    let key_ino = fs.lookup(keys, "id_ed25519").unwrap().unwrap();
    assert_eq!(fs.read(notes_ino, 0, notes.len()).unwrap(), notes);
    assert_eq!(fs.read(list_ino, 0, list.len()).unwrap(), list);
    assert_eq!(fs.read(key_ino, 0, key.len()).unwrap(), key);
    assert_eq!(
        names(&mut fs, 1),
        [".", "..", "keys", "notes.txt", "photos"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert_eq!(
        names(&mut fs, photos),
        [".", "..", "list.csv"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert_eq!(
        names(&mut fs, keys),
        [".", "..", "id_ed25519"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn carriers_still_images_after_tree() {
    let dir = tempfile::tempdir().unwrap();
    let originals = make_pool(dir.path(), 3, 768);
    format(dir.path()).unwrap();
    let mut fs = AlbumFs::open(dir.path()).unwrap();
    build_tree(&mut fs);
    fs.sync().unwrap();
    drop(fs);

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
