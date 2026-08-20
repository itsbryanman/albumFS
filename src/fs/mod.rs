mod album;
pub mod bitmap;
pub mod block;
pub mod crypto;
pub mod dirent;
pub mod format;
pub mod fuse;
pub mod inode;
pub mod stats;
pub mod superblock;

pub use album::AlbumFs;
pub use dirent::DirEntry;
pub use inode::Inode;

use thiserror::Error;

pub const PB: usize = 4096;
pub const BLOCK_HEADER: usize = 64;
pub const UB: usize = PB - BLOCK_HEADER;
pub const SB_MAGIC: [u8; 8] = *b"ALBUMFS\0";
pub const FS_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum FsError {
    #[error(transparent)]
    Codec(#[from] crate::stego::CodecError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("physical block CRC mismatch at block {0}")]
    BlockCrc(u64),
    #[error("block authentication failed: wrong passphrase or corrupted data")]
    Auth,
    #[error("encrypted markerless filesystems require an anchor image and passphrase")]
    AnchorRequired,
    #[error("superblock CRC mismatch")]
    SuperblockCrc,
    #[error("bad superblock magic")]
    BadMagic,
    #[error("no primary carrier found in pool")]
    NoPrimary,
    #[error("carrier pool does not match manifest: {0}")]
    Manifest(String),
    #[error("logical block {0} out of range")]
    BadBlock(u64),
    #[error("out of free blocks")]
    NoSpace,
    #[error("pool has no usable carriers")]
    EmptyPool,
    #[error("an AlbumFS filesystem already exists in this pool; pass --force to wipe it")]
    AlreadyFormatted,
    #[error("entry not found")]
    NotFound,
    #[error("entry already exists")]
    Exists,
    #[error("directory is not empty")]
    NotEmpty,
    #[error("not a directory")]
    NotDir,
    #[error("is a directory")]
    IsDir,
}

// Plaintext physical blocks keep the crypto fields zero. The CRC covers the
// entire usable payload, including zero padding.
pub fn build_physical_block(payload: &[u8]) -> [u8; PB] {
    assert!(payload.len() <= UB, "payload larger than usable block");
    let mut blk = [0u8; PB];
    blk[BLOCK_HEADER..BLOCK_HEADER + payload.len()].copy_from_slice(payload);
    let mut h = crc32fast::Hasher::new();
    h.update(&blk[BLOCK_HEADER..]);
    blk[40..44].copy_from_slice(&h.finalize().to_le_bytes());
    blk
}

pub fn read_physical_block(lba: u64, blk: &[u8]) -> Result<[u8; UB], FsError> {
    if blk.len() != PB {
        return Err(FsError::Manifest(format!(
            "physical block {lba} has {} bytes, expected {PB}",
            blk.len()
        )));
    }
    let stored = u32::from_le_bytes(blk[40..44].try_into().unwrap());
    let mut h = crc32fast::Hasher::new();
    h.update(&blk[BLOCK_HEADER..PB]);
    if h.finalize() != stored {
        return Err(FsError::BlockCrc(lba));
    }
    let mut ub = [0u8; UB];
    ub.copy_from_slice(&blk[BLOCK_HEADER..PB]);
    Ok(ub)
}
