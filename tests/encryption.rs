use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use albumfs::fs::format::{format_with_anchor_options, open_with_anchor};
use albumfs::fs::{build_physical_block, read_physical_block, AlbumFs, FsError, UB};
use albumfs::stego::{
    png::PngCodec, CarrierCodec, ChunkRead, ChunkWrite, Codec, CodecError, CHUNK_MAGIC,
};
use image::{Rgba, RgbaImage};

const PASSPHRASE: &str = "correct horse battery staple";

fn make_pool(dir: &Path, count: usize) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for image_index in 0..count {
        let mut image = RgbaImage::new(512, 512);
        for (pixel_index, pixel) in image.pixels_mut().enumerate() {
            let value = (pixel_index as u32)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(image_index as u32 * 97);
            *pixel = Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255]);
        }
        let path = dir.join(format!("carrier-{image_index:03}.png"));
        image.save(&path).unwrap();
        paths.push(path);
    }
    paths
}

fn names(fs: &mut AlbumFs, inode: u64) -> BTreeSet<String> {
    fs.readdir(inode)
        .unwrap()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect()
}

#[test]
fn encrypted_markerless_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 3);
    let result =
        format_with_anchor_options(dir.path(), Some(PASSPHRASE), Some(&paths[0]), false).unwrap();
    assert!(result.superblock.markerless_encrypted());
    assert_eq!(result.anchor.as_deref(), Some(paths[0].as_path()));
    assert!(result
        .superblock
        .manifest
        .iter()
        .all(|carrier| carrier.name != paths[0].file_name().unwrap().to_str().unwrap()));

    let mut fs = AlbumFs::open_with_anchor(dir.path(), &paths[0], PASSPHRASE).unwrap();
    let folder = fs.mkdir(1, "private", 0o700).unwrap();
    let file = fs.create(folder, "message.bin", 0o600).unwrap();
    let payload: Vec<u8> = (0..(UB + 37))
        .map(|index| (index.wrapping_mul(31) % 251) as u8)
        .collect();
    fs.write(file, 0, &payload).unwrap();
    fs.sync().unwrap();
    drop(fs);

    let mut reopened = AlbumFs::open_with_anchor(dir.path(), &paths[0], PASSPHRASE).unwrap();
    let folder = reopened.lookup(1, "private").unwrap().unwrap();
    let file = reopened.lookup(folder, "message.bin").unwrap().unwrap();
    assert_eq!(reopened.read(file, 0, payload.len()).unwrap(), payload);
    assert_eq!(
        names(&mut reopened, 1),
        [".", "..", "private"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert_eq!(
        names(&mut reopened, folder),
        [".", "..", "message.bin"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn wrong_anchor_or_passphrase_fails() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 3);
    format_with_anchor_options(dir.path(), Some(PASSPHRASE), Some(&paths[0]), false).unwrap();

    let wrong_passphrase = AlbumFs::open_with_anchor(dir.path(), &paths[0], "wrong passphrase")
        .err()
        .unwrap();
    assert!(matches!(wrong_passphrase, FsError::Auth));
    let wrong_anchor = AlbumFs::open_with_anchor(dir.path(), &paths[1], PASSPHRASE)
        .err()
        .unwrap();
    assert!(matches!(wrong_anchor, FsError::Auth));
}

#[test]
fn encrypted_format_guard_and_default_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 3);
    let result = format_with_anchor_options(dir.path(), Some(PASSPHRASE), None, false).unwrap();
    assert_eq!(result.anchor.as_deref(), Some(paths[0].as_path()));
    assert!(result
        .superblock
        .manifest
        .iter()
        .all(|carrier| carrier.name != "carrier-000.png"));

    let error = format_with_anchor_options(
        dir.path(),
        Some(PASSPHRASE),
        result.anchor.as_deref(),
        false,
    )
    .err()
    .unwrap();
    assert!(matches!(error, FsError::AlreadyFormatted));
}

#[test]
fn no_plaintext_magic() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 3);
    let result =
        format_with_anchor_options(dir.path(), Some(PASSPHRASE), Some(&paths[0]), false).unwrap();
    let magic = CHUNK_MAGIC.to_le_bytes();

    for path in &paths {
        let codec = Codec::for_path(path).unwrap();
        let prefix = codec.read_prefix(path, 16).unwrap();
        assert_ne!(&prefix[..4], &magic);
        assert!(matches!(
            codec.read_chunk(path, ChunkRead::Framed, &[]),
            Err(CodecError::NotACarrier) | Err(CodecError::HeaderCrc)
        ));
    }
    assert!(result.superblock.manifest.iter().all(|carrier| {
        let path = dir.path().join(&carrier.name);
        path != paths[0]
    }));
}

#[test]
fn order_is_keyed() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.png");
    let mut image = RgbaImage::new(256, 256);
    for (index, pixel) in image.pixels_mut().enumerate() {
        let value = (index as u32).wrapping_mul(1_664_525);
        *pixel = Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255]);
    }
    image.save(&base).unwrap();
    let first = dir.path().join("first.png");
    let second = dir.path().join("second.png");
    std::fs::copy(&base, &first).unwrap();
    std::fs::copy(&base, &second).unwrap();
    let payload = vec![0xa5; 2048];
    let first_key = [0x11; 32];
    let second_key = [0x22; 32];
    let codec = PngCodec;
    codec
        .write_chunk(
            &first,
            ChunkWrite::Markerless { chunk_index: 4 },
            &payload,
            &first_key,
        )
        .unwrap();
    codec
        .write_chunk(
            &second,
            ChunkWrite::Markerless { chunk_index: 4 },
            &payload,
            &second_key,
        )
        .unwrap();
    assert_ne!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap()
    );
    let (_, first_payload) = codec
        .read_chunk(
            &first,
            ChunkRead::Markerless {
                chunk_index: 4,
                payload_len: payload.len(),
            },
            &first_key,
        )
        .unwrap();
    let (_, second_payload) = codec
        .read_chunk(
            &second,
            ChunkRead::Markerless {
                chunk_index: 4,
                payload_len: payload.len(),
            },
            &second_key,
        )
        .unwrap();
    assert_eq!(first_payload, payload);
    assert_eq!(second_payload, payload);
}

#[test]
fn nonce_unique_per_write() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 3);
    let result =
        format_with_anchor_options(dir.path(), Some(PASSPHRASE), Some(&paths[0]), false).unwrap();
    let data_path = dir.path().join(&result.superblock.manifest[0].name);
    let before = std::fs::read(&data_path).unwrap();
    let (_, mut store) = open_with_anchor(dir.path(), &paths[0], PASSPHRASE).unwrap();
    let lba = result.superblock.data_start;
    store.write_block(lba, &[0x5a; UB]).unwrap();
    store.flush().unwrap();
    let first = std::fs::read(&data_path).unwrap();
    store.write_block(lba, &[0x5a; UB]).unwrap();
    store.flush().unwrap();
    let second = std::fs::read(&data_path).unwrap();
    assert_ne!(before, first);
    assert_ne!(first, second);
}

#[test]
fn tamper_detected() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 3);
    let result =
        format_with_anchor_options(dir.path(), Some(PASSPHRASE), Some(&paths[0]), false).unwrap();
    let data_path = dir.path().join(&result.superblock.manifest[0].name);
    let mut image = image::open(&data_path).unwrap().to_rgba8();
    image.get_pixel_mut(0, 0).0[0] ^= 1;
    image.save(&data_path).unwrap();

    let (_, mut store) = open_with_anchor(dir.path(), &paths[0], PASSPHRASE).unwrap();
    let mut saw_auth = false;
    for lba in 0..result.superblock.total_blocks {
        if matches!(store.read_block(lba), Err(FsError::Auth)) {
            saw_auth = true;
            break;
        }
    }
    assert!(saw_auth, "tampered carrier did not fail authentication");
}

#[test]
fn plaintext_mode_unchanged() {
    let payload: Vec<u8> = (0..UB).map(|index| (index % 251) as u8).collect();
    let block = build_physical_block(&payload);
    assert_eq!(&block[0..40], &[0; 40]);
    assert_eq!(read_physical_block(9, &block).unwrap(), payload.as_slice());
}
