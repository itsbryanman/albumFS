use albumfs::fs::dirent::DirEntry;
use albumfs::fs::inode::{Inode, INODE_SIZE};
use albumfs::fs::superblock::{CarrierRange, Superblock};
use albumfs::fs::SB_MAGIC;
use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};

#[test]
fn parsers_never_panic_on_random_or_truncated_input() {
    let mut rng = StdRng::seed_from_u64(0x00a1_b65f_5eed);
    for _ in 0..20_000 {
        let len = rng.gen_range(0..=2048);
        let mut bytes = vec![0u8; len];
        rng.fill_bytes(&mut bytes);
        let _ = Superblock::decode(&bytes);
        let _ = Inode::decode(&bytes);
        for offset in [0, len / 2, len, usize::MAX, usize::MAX - 8] {
            let _ = DirEntry::decode(&bytes, offset);
        }
    }

    let superblock = Superblock::standard(
        64,
        64,
        1,
        2,
        6,
        vec![
            CarrierRange {
                name: String::new(),
                chunk_index: 0,
                block_start: 0,
                block_count: 32,
            },
            CarrierRange {
                name: String::new(),
                chunk_index: 1,
                block_start: 32,
                block_count: 32,
            },
        ],
        false,
    )
    .encode();
    for end in 0..=superblock.len() {
        let _ = Superblock::decode(&superblock[..end]);
    }

    let markerless = Superblock::markerless(
        64,
        64,
        1,
        2,
        6,
        vec![CarrierRange {
            name: "carrier.jpg".into(),
            chunk_index: 0,
            block_start: 0,
            block_count: 64,
        }],
    )
    .encode();
    for end in 0..=markerless.len() {
        let _ = Superblock::decode(&markerless[..end]);
    }

    let inode = Inode::default().encode();
    for end in 0..=INODE_SIZE {
        let _ = Inode::decode(&inode[..end]);
    }
}

#[test]
fn parsers_reject_deliberately_hostile_lengths() {
    let mut oversized_record = [0u8; 16];
    oversized_record[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(DirEntry::decode(&oversized_record, 0).is_err());

    let mut oversized_name = [0u8; 16];
    oversized_name[8..10].copy_from_slice(&12u16.to_le_bytes());
    oversized_name[10] = u8::MAX;
    assert!(DirEntry::decode(&oversized_name, 0).is_err());

    let zero_record = [0u8; 16];
    assert!(DirEntry::decode(&zero_record, 0).is_err());
    assert!(DirEntry::decode(&zero_record, usize::MAX).is_err());

    let mut oversized_manifest = [0u8; 108];
    oversized_manifest[0..8].copy_from_slice(&SB_MAGIC);
    oversized_manifest[8..12].copy_from_slice(&2u32.to_le_bytes());
    oversized_manifest[68..72].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Superblock::decode(&oversized_manifest).is_err());
}
