use std::path::Path;
use thiserror::Error;

pub mod png;

pub const CHUNK_MAGIC: u32 = 0x414C_424D; // "ALBM"
pub const CHUNK_HEADER_LEN: usize = 16;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("payload of {payload} bytes exceeds usable capacity of {capacity} bytes")]
    PayloadTooLarge { payload: u64, capacity: u64 },
    #[error("no valid AlbumFS chunk in this carrier")]
    NotACarrier,
    #[error("chunk header CRC mismatch")]
    HeaderCrc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkMeta {
    pub chunk_index: u32,
    pub flags: u16,
}

/// A carrier stores one opaque chunk (metadata plus payload) inside an image.
/// order_key is reserved for a keyed embedding permutation in a later milestone
/// and is unused in this one.
pub trait CarrierCodec {
    fn capacity_bytes(&self, path: &Path) -> Result<u64, CodecError>;
    fn write_chunk(
        &self,
        path: &Path,
        meta: ChunkMeta,
        payload: &[u8],
        order_key: &[u8],
    ) -> Result<(), CodecError>;
    fn read_chunk(&self, path: &Path, order_key: &[u8]) -> Result<(ChunkMeta, Vec<u8>), CodecError>;
}

use crc::{Crc, CRC_16_IBM_SDLC};
const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);

// Header layout, little-endian:
//   0..4   magic (u32)
//   4..8   chunk_index (u32)
//   8..12  chunk_len (u32)
//   12..14 flags (u16)
//   14..16 crc16 over bytes 0..14 (u16)
pub(crate) fn encode_header(meta: ChunkMeta, chunk_len: u32) -> [u8; CHUNK_HEADER_LEN] {
    let mut h = [0u8; CHUNK_HEADER_LEN];
    h[0..4].copy_from_slice(&CHUNK_MAGIC.to_le_bytes());
    h[4..8].copy_from_slice(&meta.chunk_index.to_le_bytes());
    h[8..12].copy_from_slice(&chunk_len.to_le_bytes());
    h[12..14].copy_from_slice(&meta.flags.to_le_bytes());
    let crc = CRC16.checksum(&h[0..14]);
    h[14..16].copy_from_slice(&crc.to_le_bytes());
    h
}

pub(crate) fn decode_header(h: &[u8]) -> Result<(ChunkMeta, u32), CodecError> {
    if h.len() < CHUNK_HEADER_LEN {
        return Err(CodecError::NotACarrier);
    }
    // try_into on fixed-length slices cannot fail, so unwrap here is safe.
    let magic = u32::from_le_bytes(h[0..4].try_into().unwrap());
    if magic != CHUNK_MAGIC {
        return Err(CodecError::NotACarrier);
    }
    let stored = u16::from_le_bytes(h[14..16].try_into().unwrap());
    if CRC16.checksum(&h[0..14]) != stored {
        return Err(CodecError::HeaderCrc);
    }
    let chunk_index = u32::from_le_bytes(h[4..8].try_into().unwrap());
    let chunk_len = u32::from_le_bytes(h[8..12].try_into().unwrap());
    let flags = u16::from_le_bytes(h[12..14].try_into().unwrap());
    Ok((ChunkMeta { chunk_index, flags }, chunk_len))
}
