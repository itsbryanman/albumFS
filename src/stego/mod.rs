use rand::seq::SliceRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use std::path::Path;
use thiserror::Error;

pub mod jpeg;
mod jpeg_ffi;
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
    #[error("JPEG coefficient error: {0}")]
    Jpeg(String),
    #[error("invalid carrier codec mode: {0}")]
    InvalidMode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

pub fn kind_for(path: &Path) -> Option<ImageKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(ImageKind::Png),
        "jpg" | "jpeg" => Some(ImageKind::Jpeg),
        _ => None,
    }
}

pub enum Codec {
    Png(png::PngCodec),
    Jpeg(jpeg::JpegCodec),
}

impl Codec {
    pub fn for_path(path: &Path) -> Option<Self> {
        match kind_for(path)? {
            ImageKind::Png => Some(Self::Png(png::PngCodec)),
            ImageKind::Jpeg => Some(Self::Jpeg(jpeg::JpegCodec)),
        }
    }
}

impl CarrierCodec for Codec {
    fn raw_capacity_bytes(&self, path: &Path) -> Result<u64, CodecError> {
        match self {
            Self::Png(codec) => codec.raw_capacity_bytes(path),
            Self::Jpeg(codec) => codec.raw_capacity_bytes(path),
        }
    }

    fn write_chunk(
        &self,
        path: &Path,
        mode: ChunkWrite,
        payload: &[u8],
        order_key: &[u8],
    ) -> Result<(), CodecError> {
        match self {
            Self::Png(codec) => codec.write_chunk(path, mode, payload, order_key),
            Self::Jpeg(codec) => codec.write_chunk(path, mode, payload, order_key),
        }
    }

    fn read_chunk(
        &self,
        path: &Path,
        mode: ChunkRead,
        order_key: &[u8],
    ) -> Result<(ChunkMeta, Vec<u8>), CodecError> {
        match self {
            Self::Png(codec) => codec.read_chunk(path, mode, order_key),
            Self::Jpeg(codec) => codec.read_chunk(path, mode, order_key),
        }
    }

    fn write_prefix(&self, path: &Path, bytes: &[u8]) -> Result<(), CodecError> {
        match self {
            Self::Png(codec) => codec.write_prefix(path, bytes),
            Self::Jpeg(codec) => codec.write_prefix(path, bytes),
        }
    }

    fn read_prefix(&self, path: &Path, length: usize) -> Result<Vec<u8>, CodecError> {
        match self {
            Self::Png(codec) => codec.read_prefix(path, length),
            Self::Jpeg(codec) => codec.read_prefix(path, length),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkMeta {
    pub chunk_index: u32,
    pub flags: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkWrite {
    Framed(ChunkMeta),
    Markerless { chunk_index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkRead {
    Framed,
    Markerless {
        chunk_index: u32,
        payload_len: usize,
    },
}

/// A carrier stores one opaque chunk inside an image. Framed mode preserves the
/// plaintext ALBM header. Markerless mode requires a nonempty order key and
/// carries no in-band marker or length.
pub trait CarrierCodec {
    fn raw_capacity_bytes(&self, path: &Path) -> Result<u64, CodecError>;
    fn capacity_bytes(&self, path: &Path) -> Result<u64, CodecError> {
        Ok(self
            .raw_capacity_bytes(path)?
            .saturating_sub(CHUNK_HEADER_LEN as u64))
    }
    fn write_chunk(
        &self,
        path: &Path,
        mode: ChunkWrite,
        payload: &[u8],
        order_key: &[u8],
    ) -> Result<(), CodecError>;
    fn read_chunk(
        &self,
        path: &Path,
        mode: ChunkRead,
        order_key: &[u8],
    ) -> Result<(ChunkMeta, Vec<u8>), CodecError>;
    fn write_prefix(&self, path: &Path, bytes: &[u8]) -> Result<(), CodecError>;
    fn read_prefix(&self, path: &Path, length: usize) -> Result<Vec<u8>, CodecError>;
}

pub(crate) fn order_positions(
    mut positions: Vec<usize>,
    order_key: &[u8],
    chunk_index: u32,
) -> Result<Vec<usize>, CodecError> {
    if order_key.len() != 32 {
        return Err(CodecError::InvalidMode(
            "markerless mode requires a 32-byte order key".into(),
        ));
    }
    let mut material = [0u8; 36];
    material[..32].copy_from_slice(order_key);
    material[32..].copy_from_slice(&chunk_index.to_le_bytes());
    let seed = blake3::derive_key("albumfs carrier order v1", &material);
    let mut rng = ChaCha20Rng::from_seed(seed);
    positions.shuffle(&mut rng);
    Ok(positions)
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
