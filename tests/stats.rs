use std::path::Path;

use albumfs::fs::format::{format, load_bitmap, open};
use albumfs::fs::stats::{collect, render, CarrierFill, FsStats};
use image::{Rgba, RgbaImage};

fn make_pool(dir: &Path, count: usize) {
    for image_index in 0..count {
        let mut image = RgbaImage::new(512, 512);
        for (pixel_index, pixel) in image.pixels_mut().enumerate() {
            let value = (pixel_index as u32)
                .wrapping_mul(1597)
                .wrapping_add(image_index as u32 * 61);
            *pixel = Rgba([value as u8, (value >> 7) as u8, (value >> 15) as u8, 255]);
        }
        image
            .save(dir.join(format!("carrier-{image_index:03}.png")))
            .unwrap();
    }
}

#[test]
fn stats_reports_fields() {
    let dir = tempfile::tempdir().unwrap();
    make_pool(dir.path(), 2);
    let formatted = format(dir.path()).unwrap();
    let (opened, mut store) = open(dir.path()).unwrap();
    let bitmap = load_bitmap(&mut store, &opened).unwrap();
    let expected_free = bitmap.count_free();

    let stats = collect(dir.path(), None).unwrap();
    assert_eq!(stats.block_size, formatted.block_size);
    assert_eq!(stats.total_blocks, formatted.total_blocks);
    assert_eq!(stats.free_blocks, expected_free);
    assert_eq!(stats.used_inodes, 1);
    assert_eq!(stats.total_inodes, formatted.inode_count);
    assert!(!stats.encrypted);
    assert_eq!(stats.carriers.len(), formatted.manifest.len());
    assert_eq!(
        stats
            .carriers
            .iter()
            .map(|carrier| carrier.capacity_blocks)
            .sum::<u64>(),
        formatted.total_blocks
    );
    assert_eq!(
        stats
            .carriers
            .iter()
            .map(|carrier| carrier.used_blocks)
            .sum::<u64>(),
        formatted.total_blocks - expected_free
    );

    let report = render(&stats);
    for field in [
        "block size",
        "total blocks",
        "free blocks",
        "used inodes",
        "total inodes",
        "encrypted",
        "carrier count",
        "carrier 0",
    ] {
        assert!(report.contains(field), "missing field {field}");
    }
}

#[test]
fn stats_warns_below_ten_percent_free() {
    let stats = FsStats {
        block_size: 4096,
        total_blocks: 100,
        free_blocks: 9,
        used_inodes: 4,
        total_inodes: 64,
        encrypted: true,
        carriers: vec![CarrierFill {
            chunk_index: 0,
            used_blocks: 91,
            capacity_blocks: 100,
        }],
    };
    assert!(stats.low_space());
    assert!(render(&stats).contains("warning            free space is below 10 percent"));
}
