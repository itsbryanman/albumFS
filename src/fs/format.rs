use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::stego::{kind_for, CarrierCodec, ChunkRead, Codec};

use super::bitmap::Bitmap;
use super::block::BlockStore;
use super::crypto::{create_bootstrap, open_bootstrap, BOOTSTRAP_LEN};
use super::dirent::initial_directory_block;
use super::inode::{Inode, S_IFDIR};
use super::superblock::{validate_name, CarrierRange, Superblock};
use super::{read_physical_block, FsError, PB, UB};

pub struct FormatResult {
    pub superblock: Superblock,
    pub anchor: Option<PathBuf>,
}

fn carrier_paths(dir: &Path) -> Result<Vec<PathBuf>, FsError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if kind_for(&path).is_some() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn format(dir: &Path) -> Result<Superblock, FsError> {
    Ok(format_with_anchor_options(dir, None, None, false)?.superblock)
}

pub fn format_with_passphrase(dir: &Path, passphrase: Option<&str>) -> Result<Superblock, FsError> {
    Ok(format_with_anchor_options(dir, passphrase, None, false)?.superblock)
}

pub fn format_with_options(
    dir: &Path,
    passphrase: Option<&str>,
    force: bool,
) -> Result<Superblock, FsError> {
    Ok(format_with_anchor_options(dir, passphrase, None, force)?.superblock)
}

pub fn format_with_anchor_options(
    dir: &Path,
    passphrase: Option<&str>,
    requested_anchor: Option<&Path>,
    force: bool,
) -> Result<FormatResult, FsError> {
    if requested_anchor.is_some() && passphrase.is_none() {
        return Err(FsError::Manifest(
            "--anchor requires an encrypted format with a passphrase".into(),
        ));
    }
    let all_paths = carrier_paths(dir)?;
    if all_paths.is_empty() {
        return Err(FsError::EmptyPool);
    }
    let anchor = match passphrase {
        Some(_) => Some(select_anchor(dir, &all_paths, requested_anchor)?),
        None => None,
    };
    if !force && contains_valid_plaintext_filesystem(&all_paths)? {
        return Err(FsError::AlreadyFormatted);
    }
    if !force {
        if let (Some(passphrase), Some(anchor)) = (passphrase, anchor.as_deref()) {
            if contains_valid_markerless_filesystem(anchor, passphrase)? {
                return Err(FsError::AlreadyFormatted);
            }
        }
    }

    let mut usable = Vec::new();
    let mut names = HashSet::new();
    for path in all_paths {
        if anchor.as_ref().is_some_and(|anchor| *anchor == path) {
            continue;
        }
        let codec = Codec::for_path(&path).ok_or_else(|| {
            FsError::Manifest(format!("unsupported carrier type: {}", path.display()))
        })?;
        let block_count = if passphrase.is_some() {
            codec.raw_capacity_bytes(&path)? / PB as u64
        } else {
            codec.capacity_bytes(&path)? / PB as u64
        };
        if block_count == 0 {
            continue;
        }
        let name = if passphrase.is_some() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    FsError::Manifest(format!(
                        "carrier name is not valid UTF-8: {}",
                        path.display()
                    ))
                })?
                .to_owned();
            validate_name(&name)?;
            if !names.insert(name.clone()) {
                return Err(FsError::Manifest(format!(
                    "duplicate carrier name {name:?}"
                )));
            }
            name
        } else {
            String::new()
        };
        usable.push((path, name, block_count));
    }
    if usable.is_empty() {
        return Err(FsError::EmptyPool);
    }

    let mut block_start = 0u64;
    let mut paths = Vec::with_capacity(usable.len());
    let mut manifest = Vec::with_capacity(usable.len());
    for (chunk_index, (path, name, block_count)) in usable.into_iter().enumerate() {
        paths.push(path);
        manifest.push(CarrierRange {
            name,
            chunk_index: chunk_index as u32,
            block_start,
            block_count,
        });
        block_start += block_count;
    }
    let total_blocks = block_start;
    let bitmap_start = 1;
    let bitmap_blocks = total_blocks.div_ceil(8).div_ceil(UB as u64);
    let inode_table_start = bitmap_start + bitmap_blocks;
    let inode_count = 64.max(total_blocks / 4);
    let inode_table_blocks = inode_count.div_ceil(21);
    let data_start = inode_table_start + inode_table_blocks;
    if data_start >= total_blocks {
        return Err(FsError::EmptyPool);
    }

    let (superblock, bootstrap, store_keys) = match passphrase {
        Some(passphrase) => {
            let superblock = Superblock::markerless(
                total_blocks,
                inode_count,
                bitmap_start,
                inode_table_start,
                data_start,
                manifest,
            );
            let (bootstrap, keys) = create_bootstrap(passphrase, &superblock.encode())?;
            (superblock, Some(bootstrap), Some(keys))
        }
        None => (
            Superblock::standard(
                total_blocks,
                inode_count,
                bitmap_start,
                inode_table_start,
                data_start,
                manifest,
                false,
            ),
            None,
            None,
        ),
    };
    let mut store = BlockStore::new_for_format(&paths, &superblock.manifest, store_keys);
    let zero = [0u8; UB];
    for lba in 0..total_blocks {
        store.write_block(lba, &zero)?;
    }
    if !superblock.encrypted() {
        let encoded = superblock.encode();
        if encoded.len() > UB {
            return Err(FsError::Manifest(format!(
                "superblock is {} bytes and does not fit the {UB}-byte block",
                encoded.len()
            )));
        }
        store.write_block(0, &encoded)?;
    }

    let mut bitmap = Bitmap::new(total_blocks);
    for block in 0..data_start {
        bitmap.set(block);
    }
    let root_block = bitmap.alloc_from(data_start)?;
    store.write_block(root_block, &initial_directory_block(1, 1)?)?;
    let mut root = Inode {
        mode: S_IFDIR | 0o755,
        nlink: 2,
        size: UB as u64,
        block_count: 1,
        ..Inode::default()
    };
    root.direct[0] = root_block;
    let mut inode_table = store.read_block(inode_table_start)?;
    inode_table[..super::inode::INODE_SIZE].copy_from_slice(&root.encode());
    store.write_block(inode_table_start, &inode_table)?;
    write_bitmap(&mut store, &superblock, &bitmap)?;
    store.flush()?;

    if let (Some(anchor), Some(bootstrap)) = (anchor.as_deref(), bootstrap.as_ref()) {
        let codec = Codec::for_path(anchor).ok_or_else(|| {
            FsError::Manifest(format!("unsupported anchor type: {}", anchor.display()))
        })?;
        codec.write_prefix(anchor, bootstrap)?;
    }
    Ok(FormatResult { superblock, anchor })
}

fn select_anchor(
    dir: &Path,
    paths: &[PathBuf],
    requested: Option<&Path>,
) -> Result<PathBuf, FsError> {
    let anchor = if let Some(requested) = requested {
        let candidate = if requested.exists() {
            requested.to_path_buf()
        } else {
            dir.join(requested)
        };
        let canonical = fs::canonicalize(&candidate)?;
        paths
            .iter()
            .find(|path| fs::canonicalize(path).is_ok_and(|path| path == canonical))
            .cloned()
            .ok_or_else(|| {
                FsError::Manifest(format!(
                    "anchor {} is not an image carrier in {}",
                    requested.display(),
                    dir.display()
                ))
            })?
    } else {
        let mut largest = None;
        for path in paths {
            let codec = Codec::for_path(path).ok_or_else(|| {
                FsError::Manifest(format!("unsupported carrier type: {}", path.display()))
            })?;
            let capacity = codec.raw_capacity_bytes(path)?;
            if largest
                .as_ref()
                .is_none_or(|(_, largest_capacity)| capacity > *largest_capacity)
            {
                largest = Some((path.clone(), capacity));
            }
        }
        largest.map(|(path, _)| path).ok_or(FsError::EmptyPool)?
    };
    let codec = Codec::for_path(&anchor).ok_or_else(|| {
        FsError::Manifest(format!("unsupported anchor type: {}", anchor.display()))
    })?;
    let capacity = codec.raw_capacity_bytes(&anchor)?;
    if capacity < BOOTSTRAP_LEN as u64 {
        return Err(FsError::Manifest(format!(
            "anchor {} has {capacity} usable bytes but the bootstrap requires {BOOTSTRAP_LEN}",
            anchor.display()
        )));
    }
    Ok(anchor)
}

fn contains_valid_plaintext_filesystem(paths: &[PathBuf]) -> Result<bool, FsError> {
    for path in paths {
        let Some(codec) = Codec::for_path(path) else {
            continue;
        };
        let Ok((meta, payload)) = codec.read_chunk(path, ChunkRead::Framed, &[]) else {
            continue;
        };
        if meta.flags & 1 == 0 || payload.len() < PB {
            continue;
        }
        let Ok(block_zero) = read_physical_block(0, &payload[..PB]) else {
            continue;
        };
        if Superblock::decode(&block_zero).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn contains_valid_markerless_filesystem(anchor: &Path, passphrase: &str) -> Result<bool, FsError> {
    let codec = Codec::for_path(anchor).ok_or_else(|| {
        FsError::Manifest(format!("unsupported anchor type: {}", anchor.display()))
    })?;
    let prefix = codec.read_prefix(anchor, BOOTSTRAP_LEN)?;
    let Ok((plaintext, _)) = open_bootstrap(passphrase, &prefix) else {
        return Ok(false);
    };
    Ok(Superblock::decode(&plaintext).is_ok_and(|superblock| superblock.markerless_encrypted()))
}

pub fn open(dir: &Path) -> Result<(Superblock, BlockStore), FsError> {
    open_plaintext(dir)
}

pub fn open_with_passphrase(
    dir: &Path,
    passphrase: Option<&str>,
) -> Result<(Superblock, BlockStore), FsError> {
    if passphrase.is_some() {
        return Err(FsError::AnchorRequired);
    }
    open_plaintext(dir)
}

pub fn open_with_anchor(
    dir: &Path,
    anchor: &Path,
    passphrase: &str,
) -> Result<(Superblock, BlockStore), FsError> {
    let paths = carrier_paths(dir)?;
    let anchor = select_anchor(dir, &paths, Some(anchor))?;
    let codec = Codec::for_path(&anchor).ok_or_else(|| {
        FsError::Manifest(format!("unsupported anchor type: {}", anchor.display()))
    })?;
    let prefix = codec.read_prefix(&anchor, BOOTSTRAP_LEN)?;
    let (plaintext, keys) = open_bootstrap(passphrase, &prefix)?;
    let superblock = Superblock::decode(&plaintext).map_err(|_| FsError::Auth)?;
    if !superblock.markerless_encrypted() {
        return Err(FsError::Auth);
    }

    let mut seen = HashSet::new();
    let mut data_paths = Vec::with_capacity(superblock.manifest.len());
    for range in &superblock.manifest {
        validate_name(&range.name)?;
        if !seen.insert(&range.name) {
            return Err(FsError::Manifest(format!(
                "duplicate carrier name {:?}",
                range.name
            )));
        }
        let path = dir.join(&range.name);
        if fs::canonicalize(&path)? == fs::canonicalize(&anchor)? {
            return Err(FsError::Manifest(
                "anchor must not appear in the data-carrier manifest".into(),
            ));
        }
        let codec = Codec::for_path(&path).ok_or_else(|| {
            FsError::Manifest(format!("unsupported carrier type: {}", path.display()))
        })?;
        let expected = range
            .block_count
            .checked_mul(PB as u64)
            .ok_or_else(|| FsError::Manifest("carrier payload length overflow".into()))?;
        if codec.raw_capacity_bytes(&path)? < expected {
            return Err(FsError::Manifest(format!(
                "carrier {} no longer has enough capacity",
                range.name
            )));
        }
        data_paths.push((path, false));
    }
    validate_manifest_geometry(&superblock)?;
    let store = BlockStore::from_manifest(data_paths, &superblock.manifest, Some(keys));
    Ok((superblock, store))
}

fn open_plaintext(dir: &Path) -> Result<(Superblock, BlockStore), FsError> {
    let mut scanned = Vec::new();
    for path in carrier_paths(dir)? {
        let Some(codec) = Codec::for_path(&path) else {
            continue;
        };
        if let Ok((meta, payload)) = codec.read_chunk(&path, ChunkRead::Framed, &[]) {
            scanned.push((path, meta, payload));
        }
    }
    let primary = scanned
        .iter()
        .find(|(_, meta, _)| meta.flags & 1 != 0)
        .ok_or(FsError::NoPrimary)?;
    if primary.2.len() < PB {
        return Err(FsError::Manifest(
            "primary carrier is shorter than one block".into(),
        ));
    }
    let block_zero = read_physical_block(0, &primary.2[..PB])?;
    let superblock = Superblock::decode(&block_zero)?;
    if superblock.encrypted() {
        return Err(FsError::AnchorRequired);
    }
    if scanned.len() != superblock.manifest.len() {
        return Err(FsError::Manifest(format!(
            "found {} carriers, manifest names {}",
            scanned.len(),
            superblock.manifest.len()
        )));
    }

    let mut paths = Vec::with_capacity(superblock.manifest.len());
    for range in &superblock.manifest {
        let (path, meta, payload) = scanned
            .iter()
            .find(|(_, meta, _)| meta.chunk_index == range.chunk_index)
            .ok_or_else(|| {
                FsError::Manifest(format!("missing chunk index {}", range.chunk_index))
            })?;
        let expected = range.block_count as usize * PB;
        if payload.len() != expected {
            return Err(FsError::Manifest(format!(
                "chunk {} has {} bytes, expected {expected}",
                range.chunk_index,
                payload.len()
            )));
        }
        let codec = Codec::for_path(path).ok_or_else(|| {
            FsError::Manifest(format!("unsupported carrier type: {}", path.display()))
        })?;
        if codec.capacity_bytes(path)? < expected as u64 {
            return Err(FsError::Manifest(format!(
                "chunk {} no longer has enough capacity",
                range.chunk_index
            )));
        }
        paths.push((path.clone(), meta.flags & 1 != 0));
    }
    validate_manifest_geometry(&superblock)?;
    let store = BlockStore::from_manifest(paths, &superblock.manifest, None);
    Ok((superblock, store))
}

fn validate_manifest_geometry(superblock: &Superblock) -> Result<(), FsError> {
    let mut next = 0u64;
    for range in &superblock.manifest {
        if range.block_start != next || range.block_count == 0 {
            return Err(FsError::Manifest(
                "carrier manifest block ranges are not contiguous".into(),
            ));
        }
        next = next
            .checked_add(range.block_count)
            .ok_or_else(|| FsError::Manifest("carrier block range overflow".into()))?;
    }
    if next != superblock.total_blocks {
        return Err(FsError::Manifest(
            "carrier manifest total does not match superblock".into(),
        ));
    }
    Ok(())
}

pub fn load_bitmap(store: &mut BlockStore, superblock: &Superblock) -> Result<Bitmap, FsError> {
    let needed = superblock.total_blocks.div_ceil(8) as usize;
    let mut bytes = Vec::with_capacity(needed);
    for block in superblock.bitmap_start..superblock.inode_table_start {
        let payload = store.read_block(block)?;
        let remaining = needed.saturating_sub(bytes.len());
        bytes.extend_from_slice(&payload[..remaining.min(UB)]);
    }
    Ok(Bitmap::from_bytes(&bytes, superblock.total_blocks))
}

pub fn write_bitmap(
    store: &mut BlockStore,
    superblock: &Superblock,
    bitmap: &Bitmap,
) -> Result<(), FsError> {
    for (index, block) in (superblock.bitmap_start..superblock.inode_table_start).enumerate() {
        let start = index * UB;
        let end = (start + UB).min(bitmap.as_bytes().len());
        let slice = if start < end {
            &bitmap.as_bytes()[start..end]
        } else {
            &[]
        };
        store.write_block(block, slice)?;
    }
    Ok(())
}
