use std::path::Path;

use albumfs::stego::{jpeg::JpegCodec, CarrierCodec, ChunkMeta, ChunkRead, ChunkWrite, CodecError};
use image::{DynamicImage, Rgba, RgbaImage};

fn make_jpeg(path: &Path, width: u32, height: u32, seed: u32) {
    let mut image = RgbaImage::new(width, height);
    let mut state = seed;
    for (index, pixel) in image.pixels_mut().enumerate() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let x = index as u32 % width;
        let y = index as u32 / width;
        let noise = (state >> 24) as u8;
        *pixel = Rgba([
            (x as u8).wrapping_add(noise / 2),
            (y as u8).wrapping_add(noise / 3),
            ((x + y) as u8).wrapping_add(noise / 4),
            255,
        ]);
    }
    DynamicImage::ImageRgba8(image).save(path).unwrap();
}

#[test]
fn jpeg_coefficient_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("carrier.jpg");
    make_jpeg(&path, 512, 512, 1);
    let codec = JpegCodec;
    let capacity = codec.capacity_bytes(&path).unwrap();
    assert!(capacity > 2048, "JPEG capacity was only {capacity} bytes");
    let payload: Vec<u8> = (0..2048).map(|index| (index * 31 % 251) as u8).collect();
    let meta = ChunkMeta {
        chunk_index: 9,
        flags: 3,
    };
    codec
        .write_chunk(&path, ChunkWrite::Framed(meta), &payload, &[])
        .unwrap();
    let (read_meta, read_payload) = codec.read_chunk(&path, ChunkRead::Framed, &[]).unwrap();
    assert_eq!(read_meta, meta);
    assert_eq!(read_payload, payload);
}

#[test]
fn jpeg_capacity_stable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stable.jpeg");
    make_jpeg(&path, 512, 512, 2);
    let codec = JpegCodec;
    let before = codec.capacity_bytes(&path).unwrap();
    codec
        .write_chunk(
            &path,
            ChunkWrite::Framed(ChunkMeta {
                chunk_index: 1,
                flags: 0,
            }),
            &[0x5a; 1024],
            &[],
        )
        .unwrap();
    let after = codec.capacity_bytes(&path).unwrap();
    assert_eq!(before, after);
}

#[test]
fn jpeg_not_a_carrier() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.jpg");
    make_jpeg(&plain, 256, 256, 3);
    let codec = JpegCodec;
    assert!(matches!(
        codec.read_chunk(&plain, ChunkRead::Framed, &[]),
        Err(CodecError::NotACarrier) | Err(CodecError::HeaderCrc)
    ));

    let malformed = dir.path().join("malformed.jpg");
    std::fs::write(&malformed, [0xff, 0xd8, 0xff, 0xdb, 0x00]).unwrap();
    assert!(codec
        .read_chunk(&malformed, ChunkRead::Framed, &[])
        .is_err());
}

#[test]
fn jpeg_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("visible.jpg");
    make_jpeg(&path, 512, 512, 4);
    let before = image::open(&path).unwrap().to_rgb8();
    let codec = JpegCodec;
    codec
        .write_chunk(
            &path,
            ChunkWrite::Framed(ChunkMeta {
                chunk_index: 0,
                flags: 1,
            }),
            &[0xa6; 1024],
            &[],
        )
        .unwrap();
    let after = image::open(&path).unwrap().to_rgb8();
    assert_eq!(before.dimensions(), after.dimensions());
    let difference: u64 = before
        .pixels()
        .zip(after.pixels())
        .map(|(old, new)| {
            (0..3)
                .map(|channel| old.0[channel].abs_diff(new.0[channel]) as u64)
                .sum::<u64>()
        })
        .sum();
    let samples = u64::from(before.width()) * u64::from(before.height()) * 3;
    let mean_difference = difference as f64 / samples as f64;
    assert!(
        mean_difference < 3.0,
        "mean pixel difference was {mean_difference}"
    );
}
