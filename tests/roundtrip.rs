use albumfs::stego::{png::PngCodec, CarrierCodec, ChunkMeta, ChunkRead, ChunkWrite, CodecError};
use image::{Rgba, RgbaImage};
use rand::Rng;
use std::path::{Path, PathBuf};

fn make_carrier(dir: &Path, w: u32, h: u32) -> PathBuf {
    let mut img = RgbaImage::new(w, h);
    let mut rng = rand::thread_rng();
    for p in img.pixels_mut() {
        *p = Rgba([rng.gen(), rng.gen(), rng.gen(), 255]);
    }
    let path = dir.join("carrier.png");
    img.save(&path).unwrap();
    path
}

#[test]
fn roundtrip_various_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let codec = PngCodec;
    let cap = codec
        .capacity_bytes(&make_carrier(dir.path(), 200, 200))
        .unwrap();

    for n in [0usize, 1, 64, (cap / 2) as usize, cap as usize] {
        let p = make_carrier(dir.path(), 200, 200);
        let mut payload = vec![0u8; n];
        rand::thread_rng().fill(&mut payload[..]);
        let meta = ChunkMeta {
            chunk_index: 7,
            flags: 1,
        };
        codec
            .write_chunk(&p, ChunkWrite::Framed(meta), &payload, &[])
            .unwrap();
        let (rm, rp) = codec.read_chunk(&p, ChunkRead::Framed, &[]).unwrap();
        assert_eq!(rm, meta, "meta mismatch at size {n}");
        assert_eq!(rp, payload, "payload mismatch at size {n}");
    }
}

#[test]
fn payload_too_large_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = make_carrier(dir.path(), 64, 64);
    let codec = PngCodec;
    let cap = codec.capacity_bytes(&p).unwrap();
    let payload = vec![0u8; cap as usize + 1];
    let err = codec
        .write_chunk(
            &p,
            ChunkWrite::Framed(ChunkMeta {
                chunk_index: 0,
                flags: 0,
            }),
            &payload,
            &[],
        )
        .unwrap_err();
    assert!(matches!(err, CodecError::PayloadTooLarge { .. }));
}

#[test]
fn plain_image_is_not_a_carrier() {
    let dir = tempfile::tempdir().unwrap();
    let p = make_carrier(dir.path(), 64, 64);
    let codec = PngCodec;
    let err = codec.read_chunk(&p, ChunkRead::Framed, &[]).unwrap_err();
    assert!(matches!(err, CodecError::NotACarrier));
}
