use std::path::{Path, PathBuf};

use crate::stego::{CarrierCodec, ChunkMeta, ChunkRead, ChunkWrite, Codec};

use super::crypto::{BlockCipher, Key, StoreKeys};
use super::format;
use super::superblock::{CarrierRange, Superblock};
use super::{build_physical_block, read_physical_block, FsError, PB, UB};

pub struct BlockStore {
    carriers: Vec<CarrierSlot>,
    total_blocks: u64,
    encrypted: bool,
    cipher: Option<BlockCipher>,
    order_key: Option<Key>,
}

pub(crate) struct CarrierSlot {
    path: PathBuf,
    chunk_index: u32,
    is_primary: bool,
    block_start: u64,
    block_count: u64,
    buffer: Option<Vec<u8>>,
    dirty: bool,
}

impl BlockStore {
    pub fn open(dir: &Path) -> Result<(Superblock, Self), FsError> {
        format::open(dir)
    }

    pub fn open_with_passphrase(
        dir: &Path,
        passphrase: Option<&str>,
    ) -> Result<(Superblock, Self), FsError> {
        format::open_with_passphrase(dir, passphrase)
    }

    pub fn open_with_anchor(
        dir: &Path,
        anchor: &Path,
        passphrase: &str,
    ) -> Result<(Superblock, Self), FsError> {
        format::open_with_anchor(dir, anchor, passphrase)
    }

    pub(crate) fn new_for_format(
        paths: &[PathBuf],
        manifest: &[CarrierRange],
        keys: Option<StoreKeys>,
    ) -> Self {
        let carriers = paths
            .iter()
            .zip(manifest)
            .map(|(path, range)| CarrierSlot {
                path: path.clone(),
                chunk_index: range.chunk_index,
                is_primary: range.chunk_index == 0,
                block_start: range.block_start,
                block_count: range.block_count,
                buffer: Some(vec![0u8; range.block_count as usize * PB]),
                dirty: true,
            })
            .collect();
        let total_blocks = manifest.iter().map(|range| range.block_count).sum();
        let (encrypted, cipher, order_key) = match keys {
            Some(keys) => (true, Some(keys.cipher), Some(keys.order)),
            None => (false, None, None),
        };
        Self {
            carriers,
            total_blocks,
            encrypted,
            cipher,
            order_key,
        }
    }

    pub(crate) fn from_manifest(
        paths: Vec<(PathBuf, bool)>,
        manifest: &[CarrierRange],
        keys: Option<StoreKeys>,
    ) -> Self {
        let carriers = paths
            .into_iter()
            .zip(manifest)
            .map(|((path, is_primary), range)| CarrierSlot {
                path,
                chunk_index: range.chunk_index,
                is_primary,
                block_start: range.block_start,
                block_count: range.block_count,
                buffer: None,
                dirty: false,
            })
            .collect();
        let total_blocks = manifest.iter().map(|range| range.block_count).sum();
        let (encrypted, cipher, order_key) = match keys {
            Some(keys) => (true, Some(keys.cipher), Some(keys.order)),
            None => (false, None, None),
        };
        Self {
            carriers,
            total_blocks,
            encrypted,
            cipher,
            order_key,
        }
    }

    fn carrier_for(&self, lba: u64) -> Result<usize, FsError> {
        if lba >= self.total_blocks {
            return Err(FsError::BadBlock(lba));
        }
        self.carriers
            .iter()
            .position(|slot| lba >= slot.block_start && lba < slot.block_start + slot.block_count)
            .ok_or(FsError::BadBlock(lba))
    }

    fn ensure_loaded(&mut self, slot_idx: usize) -> Result<(), FsError> {
        let slot = &mut self.carriers[slot_idx];
        if slot.buffer.is_none() {
            let codec = Codec::for_path(&slot.path).ok_or_else(|| {
                FsError::Manifest(format!("unsupported carrier type: {}", slot.path.display()))
            })?;
            let expected = slot.block_count as usize * PB;
            let (mode, order_key) = if self.encrypted {
                (
                    ChunkRead::Markerless {
                        chunk_index: slot.chunk_index,
                        payload_len: expected,
                    },
                    self.order_key
                        .as_ref()
                        .ok_or(FsError::Auth)?
                        .as_bytes()
                        .as_slice(),
                )
            } else {
                (ChunkRead::Framed, &[][..])
            };
            let (meta, payload) = codec.read_chunk(&slot.path, mode, order_key)?;
            if meta.chunk_index != slot.chunk_index {
                return Err(FsError::Manifest(format!(
                    "carrier {} reports chunk index {}",
                    slot.path.display(),
                    meta.chunk_index
                )));
            }
            if payload.len() != expected {
                return Err(FsError::Manifest(format!(
                    "carrier {} has {} payload bytes, expected {expected}",
                    slot.path.display(),
                    payload.len()
                )));
            }
            slot.buffer = Some(payload);
        }
        Ok(())
    }

    pub fn read_block(&mut self, lba: u64) -> Result<[u8; UB], FsError> {
        let slot_idx = self.carrier_for(lba)?;
        self.ensure_loaded(slot_idx)?;
        let slot = &self.carriers[slot_idx];
        let offset = (lba - slot.block_start) as usize * PB;
        let buffer = slot
            .buffer
            .as_ref()
            .ok_or_else(|| FsError::Manifest("carrier buffer was not loaded".into()))?;
        let block = &buffer[offset..offset + PB];
        if self.encrypted {
            self.cipher
                .as_ref()
                .ok_or(FsError::Auth)?
                .decode(lba, block)
        } else {
            read_physical_block(lba, block)
        }
    }

    pub fn write_block(&mut self, lba: u64, payload: &[u8]) -> Result<(), FsError> {
        if payload.len() > UB {
            return Err(FsError::Manifest(format!(
                "block payload has {} bytes, maximum is {UB}",
                payload.len()
            )));
        }
        let slot_idx = self.carrier_for(lba)?;
        self.ensure_loaded(slot_idx)?;
        let slot = &mut self.carriers[slot_idx];
        let offset = (lba - slot.block_start) as usize * PB;
        let block = if self.encrypted {
            self.cipher
                .as_ref()
                .ok_or(FsError::Auth)?
                .encode(lba, payload)?
        } else {
            build_physical_block(payload)
        };
        let buffer = slot
            .buffer
            .as_mut()
            .ok_or_else(|| FsError::Manifest("carrier buffer was not loaded".into()))?;
        buffer[offset..offset + PB].copy_from_slice(&block);
        slot.dirty = true;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), FsError> {
        for slot in &mut self.carriers {
            if slot.dirty {
                let buffer = slot
                    .buffer
                    .as_ref()
                    .ok_or_else(|| FsError::Manifest("dirty carrier has no buffer".into()))?;
                let codec = Codec::for_path(&slot.path).ok_or_else(|| {
                    FsError::Manifest(format!("unsupported carrier type: {}", slot.path.display()))
                })?;
                let (mode, order_key) = if self.encrypted {
                    (
                        ChunkWrite::Markerless {
                            chunk_index: slot.chunk_index,
                        },
                        self.order_key
                            .as_ref()
                            .ok_or(FsError::Auth)?
                            .as_bytes()
                            .as_slice(),
                    )
                } else {
                    (
                        ChunkWrite::Framed(ChunkMeta {
                            chunk_index: slot.chunk_index,
                            flags: u16::from(slot.is_primary),
                        }),
                        &[][..],
                    )
                };
                codec.write_chunk(&slot.path, mode, buffer, order_key)?;
                slot.dirty = false;
            }
        }
        Ok(())
    }

    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }
}
