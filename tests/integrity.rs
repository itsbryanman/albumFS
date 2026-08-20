use albumfs::stego::{png::PngCodec, CarrierCodec, ChunkMeta, ChunkWrite};
use image::{Rgba, RgbaImage};
use rand::Rng;

#[test]
fn only_lsbs_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.png");
    let mut img = RgbaImage::new(128, 128);
    let mut rng = rand::thread_rng();
    for p in img.pixels_mut() {
        *p = Rgba([rng.gen(), rng.gen(), rng.gen(), 255]);
    }
    img.save(&path).unwrap();
    let before = image::open(&path).unwrap().to_rgba8();

    let codec = PngCodec;
    let payload: Vec<u8> = (0..500).map(|_| rng.gen()).collect();
    codec
        .write_chunk(
            &path,
            ChunkWrite::Framed(ChunkMeta {
                chunk_index: 0,
                flags: 1,
            }),
            &payload,
            &[],
        )
        .unwrap();
    let after = image::open(&path).unwrap().to_rgba8();

    assert_eq!(before.dimensions(), after.dimensions());
    for (b, a) in before.pixels().zip(after.pixels()) {
        for ch in 0..3 {
            assert_eq!(b.0[ch] >> 1, a.0[ch] >> 1, "high 7 bits changed");
        }
        assert_eq!(b.0[3], a.0[3], "alpha changed");
    }
}
