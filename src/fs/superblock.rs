use super::crypto::{ARGON2_ITERS, ARGON2_MEM_KIB, ARGON2_PARALLEL};
use super::{FsError, FS_VERSION, PB, SB_MAGIC, UB};

pub const CRYPTO_FLAG_ENCRYPTED: u32 = 1;
pub const MARKERLESS_FS_VERSION: u32 = 3;
const V1_HEADER_LEN: usize = 80;
const V2_HEADER_LEN: usize = 108;
const V3_HEADER_LEN: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierRange {
    pub name: String,
    pub chunk_index: u32,
    pub block_start: u64,
    pub block_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superblock {
    pub version: u32,
    pub block_size: u32,
    pub usable_size: u32,
    pub total_blocks: u64,
    pub inode_count: u64,
    pub bitmap_start: u64,
    pub inode_table_start: u64,
    pub data_start: u64,
    pub root_inode: u64,
    pub crypto_flags: u32,
    pub salt: [u8; 16],
    pub argon2_mem_kib: u32,
    pub argon2_iters: u32,
    pub argon2_parallel: u32,
    pub manifest: Vec<CarrierRange>,
}

impl Superblock {
    pub fn encode(&self) -> Vec<u8> {
        let header_len = match self.version {
            1 => V1_HEADER_LEN,
            2 => V2_HEADER_LEN,
            _ => V3_HEADER_LEN,
        };
        let manifest_len = if self.version >= MARKERLESS_FS_VERSION {
            self.manifest
                .iter()
                .map(|carrier| 2 + carrier.name.len() + 20)
                .sum()
        } else {
            20 * self.manifest.len()
        };
        let mut bytes = vec![0u8; header_len + manifest_len];
        bytes[0..8].copy_from_slice(&SB_MAGIC);
        bytes[8..12].copy_from_slice(&self.version.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.block_size.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.usable_size.to_le_bytes());
        bytes[20..28].copy_from_slice(&self.total_blocks.to_le_bytes());
        bytes[28..36].copy_from_slice(&self.inode_count.to_le_bytes());
        bytes[36..44].copy_from_slice(&self.bitmap_start.to_le_bytes());
        bytes[44..52].copy_from_slice(&self.inode_table_start.to_le_bytes());
        bytes[52..60].copy_from_slice(&self.data_start.to_le_bytes());
        bytes[60..68].copy_from_slice(&self.root_inode.to_le_bytes());
        bytes[68..72].copy_from_slice(&(self.manifest.len() as u32).to_le_bytes());
        bytes[72..76].copy_from_slice(&self.crypto_flags.to_le_bytes());
        if self.version == 2 {
            bytes[80..96].copy_from_slice(&self.salt);
            bytes[96..100].copy_from_slice(&self.argon2_mem_kib.to_le_bytes());
            bytes[100..104].copy_from_slice(&self.argon2_iters.to_le_bytes());
            bytes[104..108].copy_from_slice(&self.argon2_parallel.to_le_bytes());
        }
        let mut offset = header_len;
        for carrier in &self.manifest {
            if self.version >= MARKERLESS_FS_VERSION {
                let name_len = carrier.name.len() as u16;
                bytes[offset..offset + 2].copy_from_slice(&name_len.to_le_bytes());
                offset += 2;
                bytes[offset..offset + carrier.name.len()].copy_from_slice(carrier.name.as_bytes());
                offset += carrier.name.len();
            }
            bytes[offset..offset + 4].copy_from_slice(&carrier.chunk_index.to_le_bytes());
            bytes[offset + 4..offset + 12].copy_from_slice(&carrier.block_start.to_le_bytes());
            bytes[offset + 12..offset + 20].copy_from_slice(&carrier.block_count.to_le_bytes());
            offset += 20;
        }
        let mut crc = crc32fast::Hasher::new();
        crc.update(&bytes);
        bytes[76..80].copy_from_slice(&crc.finalize().to_le_bytes());
        bytes
    }

    pub fn decode(buf: &[u8]) -> Result<Self, FsError> {
        if buf.len() < V1_HEADER_LEN || buf[0..8] != SB_MAGIC {
            return Err(FsError::BadMagic);
        }
        let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        let header_len = match version {
            1 => V1_HEADER_LEN,
            2 => V2_HEADER_LEN,
            MARKERLESS_FS_VERSION => V3_HEADER_LEN,
            other => {
                return Err(FsError::Manifest(format!(
                    "unsupported filesystem version {other}"
                )))
            }
        };
        if buf.len() < header_len {
            return Err(FsError::Manifest("truncated superblock header".into()));
        }
        let image_count = u32::from_le_bytes(buf[68..72].try_into().unwrap()) as usize;
        let mut manifest = Vec::new();
        let mut offset = header_len;
        for _ in 0..image_count {
            let name = if version >= MARKERLESS_FS_VERSION {
                let name_len_end = offset
                    .checked_add(2)
                    .ok_or_else(|| FsError::Manifest("manifest offset overflow".into()))?;
                if name_len_end > buf.len() {
                    return Err(FsError::Manifest("truncated manifest name length".into()));
                }
                let name_len =
                    u16::from_le_bytes(buf[offset..name_len_end].try_into().unwrap()) as usize;
                offset = name_len_end;
                let name_end = offset
                    .checked_add(name_len)
                    .ok_or_else(|| FsError::Manifest("manifest name overflow".into()))?;
                if name_end > buf.len() {
                    return Err(FsError::Manifest("truncated manifest name".into()));
                }
                let name = std::str::from_utf8(&buf[offset..name_end])
                    .map_err(|_| FsError::Manifest("manifest name is not UTF-8".into()))?
                    .to_owned();
                validate_name(&name)?;
                offset = name_end;
                name
            } else {
                String::new()
            };
            let entry_end = offset
                .checked_add(20)
                .ok_or_else(|| FsError::Manifest("manifest entry overflow".into()))?;
            if entry_end > buf.len() {
                return Err(FsError::Manifest("truncated manifest".into()));
            }
            manifest.push(CarrierRange {
                name,
                chunk_index: u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()),
                block_start: u64::from_le_bytes(buf[offset + 4..offset + 12].try_into().unwrap()),
                block_count: u64::from_le_bytes(buf[offset + 12..offset + 20].try_into().unwrap()),
            });
            offset = entry_end;
        }
        let stored = u32::from_le_bytes(buf[76..80].try_into().unwrap());
        let mut check = buf[..offset].to_vec();
        check[76..80].fill(0);
        let mut crc = crc32fast::Hasher::new();
        crc.update(&check);
        if crc.finalize() != stored {
            return Err(FsError::SuperblockCrc);
        }
        Ok(Self {
            version,
            block_size: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            usable_size: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            total_blocks: u64::from_le_bytes(buf[20..28].try_into().unwrap()),
            inode_count: u64::from_le_bytes(buf[28..36].try_into().unwrap()),
            bitmap_start: u64::from_le_bytes(buf[36..44].try_into().unwrap()),
            inode_table_start: u64::from_le_bytes(buf[44..52].try_into().unwrap()),
            data_start: u64::from_le_bytes(buf[52..60].try_into().unwrap()),
            root_inode: u64::from_le_bytes(buf[60..68].try_into().unwrap()),
            crypto_flags: u32::from_le_bytes(buf[72..76].try_into().unwrap()),
            salt: if version == 2 {
                buf[80..96].try_into().unwrap()
            } else {
                [0; 16]
            },
            argon2_mem_kib: if version == 2 {
                u32::from_le_bytes(buf[96..100].try_into().unwrap())
            } else {
                0
            },
            argon2_iters: if version == 2 {
                u32::from_le_bytes(buf[100..104].try_into().unwrap())
            } else {
                0
            },
            argon2_parallel: if version == 2 {
                u32::from_le_bytes(buf[104..108].try_into().unwrap())
            } else {
                0
            },
            manifest,
        })
    }

    pub fn standard(
        total_blocks: u64,
        inode_count: u64,
        bitmap_start: u64,
        inode_table_start: u64,
        data_start: u64,
        manifest: Vec<CarrierRange>,
        encrypted: bool,
    ) -> Self {
        Self {
            version: FS_VERSION,
            block_size: PB as u32,
            usable_size: UB as u32,
            total_blocks,
            inode_count,
            bitmap_start,
            inode_table_start,
            data_start,
            root_inode: 1,
            crypto_flags: if encrypted { CRYPTO_FLAG_ENCRYPTED } else { 0 },
            salt: [0; 16],
            argon2_mem_kib: ARGON2_MEM_KIB,
            argon2_iters: ARGON2_ITERS,
            argon2_parallel: ARGON2_PARALLEL,
            manifest,
        }
    }

    pub fn markerless(
        total_blocks: u64,
        inode_count: u64,
        bitmap_start: u64,
        inode_table_start: u64,
        data_start: u64,
        manifest: Vec<CarrierRange>,
    ) -> Self {
        let mut superblock = Self::standard(
            total_blocks,
            inode_count,
            bitmap_start,
            inode_table_start,
            data_start,
            manifest,
            true,
        );
        superblock.version = MARKERLESS_FS_VERSION;
        superblock.argon2_mem_kib = 0;
        superblock.argon2_iters = 0;
        superblock.argon2_parallel = 0;
        superblock
    }

    pub fn encrypted(&self) -> bool {
        self.crypto_flags & CRYPTO_FLAG_ENCRYPTED != 0
    }

    pub fn markerless_encrypted(&self) -> bool {
        self.version == MARKERLESS_FS_VERSION && self.encrypted()
    }
}

pub fn validate_name(name: &str) -> Result<(), FsError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.len() > u16::MAX as usize
    {
        return Err(FsError::Manifest(format!(
            "invalid carrier manifest name {name:?}"
        )));
    }
    Ok(())
}
