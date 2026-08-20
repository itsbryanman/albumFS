use super::{FsError, UB};

pub const FILE_TYPE_FILE: u8 = 1;
pub const FILE_TYPE_DIR: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub inode: u64,
    pub rec_len: u16,
    pub name: String,
    pub file_type: u8,
}

impl DirEntry {
    pub fn required_len(name_len: usize) -> usize {
        (12 + name_len).div_ceil(4) * 4
    }

    pub fn new(inode: u64, name: &str, file_type: u8, rec_len: usize) -> Result<Self, FsError> {
        if name.len() > u8::MAX as usize
            || rec_len < Self::required_len(name.len())
            || rec_len > u16::MAX as usize
        {
            return Err(FsError::Manifest("invalid directory entry size".into()));
        }
        Ok(Self {
            inode,
            rec_len: rec_len as u16,
            name: name.to_owned(),
            file_type,
        })
    }

    pub fn encode_into(&self, target: &mut [u8]) -> Result<(), FsError> {
        let rec_len = self.rec_len as usize;
        if target.len() < rec_len || rec_len < Self::required_len(self.name.len()) {
            return Err(FsError::Manifest("directory entry does not fit".into()));
        }
        target[..rec_len].fill(0);
        target[0..8].copy_from_slice(&self.inode.to_le_bytes());
        target[8..10].copy_from_slice(&self.rec_len.to_le_bytes());
        target[10] = self.name.len() as u8;
        target[11] = self.file_type;
        target[12..12 + self.name.len()].copy_from_slice(self.name.as_bytes());
        Ok(())
    }

    pub fn decode(block: &[u8], offset: usize) -> Result<Self, FsError> {
        let header_end = offset
            .checked_add(12)
            .ok_or_else(|| FsError::Manifest("directory entry offset overflow".into()))?;
        if header_end > block.len() {
            return Err(FsError::Manifest("truncated directory entry".into()));
        }
        let inode = u64::from_le_bytes(block[offset..offset + 8].try_into().unwrap());
        let rec_len = u16::from_le_bytes(block[offset + 8..offset + 10].try_into().unwrap());
        let name_len = block[offset + 10] as usize;
        let end = offset
            .checked_add(rec_len as usize)
            .ok_or_else(|| FsError::Manifest("directory entry record overflow".into()))?;
        if rec_len == 0
            || !(rec_len as usize).is_multiple_of(4)
            || end > block.len()
            || 12 + name_len > rec_len as usize
        {
            return Err(FsError::Manifest("invalid directory entry record".into()));
        }
        let name_end = header_end
            .checked_add(name_len)
            .ok_or_else(|| FsError::Manifest("directory entry name overflow".into()))?;
        let name = std::str::from_utf8(&block[header_end..name_end])
            .map_err(|_| FsError::Manifest("directory entry name is not UTF-8".into()))?
            .to_owned();
        Ok(Self {
            inode,
            rec_len,
            name,
            file_type: block[offset + 11],
        })
    }
}

pub fn initial_directory_block(ino: u64, parent: u64) -> Result<[u8; UB], FsError> {
    let mut block = [0u8; UB];
    let dot_len = DirEntry::required_len(1);
    let dot = DirEntry::new(ino, ".", FILE_TYPE_DIR, dot_len)?;
    dot.encode_into(&mut block[..dot_len])?;
    let dotdot = DirEntry::new(parent, "..", FILE_TYPE_DIR, UB - dot_len)?;
    dotdot.encode_into(&mut block[dot_len..])?;
    Ok(block)
}
