use super::{FsError, UB};

pub const INODE_SIZE: usize = 192;
pub const INODES_PER_BLOCK: usize = UB / INODE_SIZE;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFMT: u32 = 0o170000;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inode {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub block_count: u32,
    pub direct: [u64; 12],
    pub single_indirect: u64,
    pub double_indirect: u64,
}

impl Inode {
    pub fn encode(&self) -> [u8; INODE_SIZE] {
        let mut bytes = [0u8; INODE_SIZE];
        bytes[0..4].copy_from_slice(&self.mode.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.uid.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.gid.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.nlink.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.size.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.atime.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.mtime.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.ctime.to_le_bytes());
        bytes[48..52].copy_from_slice(&self.block_count.to_le_bytes());
        for (index, pointer) in self.direct.iter().enumerate() {
            let start = 56 + index * 8;
            bytes[start..start + 8].copy_from_slice(&pointer.to_le_bytes());
        }
        bytes[152..160].copy_from_slice(&self.single_indirect.to_le_bytes());
        bytes[160..168].copy_from_slice(&self.double_indirect.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FsError> {
        if bytes.len() < INODE_SIZE {
            return Err(FsError::Manifest("truncated inode".into()));
        }
        let mut direct = [0u64; 12];
        for (index, pointer) in direct.iter_mut().enumerate() {
            let start = 56 + index * 8;
            *pointer = u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap());
        }
        Ok(Self {
            mode: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            uid: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            gid: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            nlink: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            size: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            atime: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            mtime: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            ctime: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
            block_count: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
            direct,
            single_indirect: u64::from_le_bytes(bytes[152..160].try_into().unwrap()),
            double_indirect: u64::from_le_bytes(bytes[160..168].try_into().unwrap()),
        })
    }

    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }

    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }
}
