use std::path::{Path, PathBuf};

use albumfs::fs::bitmap::Bitmap;
use albumfs::fs::format::{format, open};
use albumfs::fs::{FsError, BLOCK_HEADER, PB, UB};
use albumfs::stego::{png::PngCodec, CarrierCodec, ChunkMeta, ChunkRead, ChunkWrite};
use image::{Rgba, RgbaImage};
use rand::Rng;

fn make_pool(dir: &Path, count: usize, size: u32) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut rng = rand::thread_rng();
    for index in 0..count {
        let mut image = RgbaImage::new(size, size);
        for pixel in image.pixels_mut() {
            *pixel = Rgba([rng.gen(), rng.gen(), rng.gen(), 255]);
        }
        let path = dir.join(format!("carrier-{index:03}.png"));
        image.save(&path).unwrap();
        paths.push(path);
    }
    paths
}

#[test]
fn block_roundtrip_survives_reload() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 3, 512);
    let sb = format(dir.path()).unwrap();
    let (_, mut store) = open(dir.path()).unwrap();
    let blocks = [sb.data_start, sb.data_start + 2, sb.total_blocks - 1];
    let payloads = [vec![0x11; UB], vec![0x5a; 777], vec![0xe3; UB]];
    for (block, payload) in blocks.into_iter().zip(&payloads) {
        store.write_block(block, payload).unwrap();
    }
    store.flush().unwrap();
    drop(store);

    let (_, mut reopened) = open(dir.path()).unwrap();
    for (block, payload) in blocks.into_iter().zip(&payloads) {
        let read = reopened.read_block(block).unwrap();
        assert_eq!(&read[..payload.len()], payload);
        assert!(read[payload.len()..].iter().all(|byte| *byte == 0));
    }
}

#[test]
fn crc_detects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let paths = make_pool(dir.path(), 2, 512);
    let sb = format(dir.path()).unwrap();
    let lba = sb.data_start;
    let (_, mut store) = open(dir.path()).unwrap();
    store.write_block(lba, &[0x42; UB]).unwrap();
    store.flush().unwrap();
    drop(store);

    let range = sb
        .manifest
        .iter()
        .find(|range| lba >= range.block_start && lba < range.block_start + range.block_count)
        .unwrap();
    let codec = PngCodec;
    let path = &paths[range.chunk_index as usize];
    let (meta, mut chunk) = codec.read_chunk(path, ChunkRead::Framed, &[]).unwrap();
    let offset = (lba - range.block_start) as usize * PB + BLOCK_HEADER;
    chunk[offset] ^= 0x01;
    codec
        .write_chunk(
            path,
            ChunkWrite::Framed(ChunkMeta {
                chunk_index: meta.chunk_index,
                flags: meta.flags,
            }),
            &chunk,
            &[],
        )
        .unwrap();

    let (_, mut reopened) = open(dir.path()).unwrap();
    let error = reopened.read_block(lba).unwrap_err();
    assert!(matches!(error, FsError::BlockCrc(block) if block == lba));
}

#[test]
fn bitmap_alloc_free() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 2, 512);
    let sb = format(dir.path()).unwrap();
    let mut bitmap = Bitmap::new(sb.total_blocks);
    let baseline = bitmap.count_free();
    let allocated: Vec<_> = (0..10)
        .map(|_| bitmap.alloc_from(sb.data_start).unwrap())
        .collect();
    assert_eq!(bitmap.count_free(), baseline - 10);
    for block in allocated.into_iter().take(4) {
        bitmap.clear(block);
    }
    assert_eq!(bitmap.count_free(), baseline - 6);
}
