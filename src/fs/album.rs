use std::time::{SystemTime, UNIX_EPOCH};

use super::bitmap::Bitmap;
use super::block::BlockStore;
use super::dirent::{DirEntry, FILE_TYPE_DIR, FILE_TYPE_FILE};
use super::format::{load_bitmap, open, open_with_anchor, open_with_passphrase, write_bitmap};
use super::inode::{Inode, INODE_SIZE, S_IFDIR, S_IFREG};
use super::superblock::Superblock;
use super::{FsError, UB};

const POINTERS_PER_BLOCK: u64 = (UB / 8) as u64;

pub struct AlbumFs {
    pub(crate) store: BlockStore,
    pub(crate) sb: Superblock,
    pub(crate) bitmap: Bitmap,
}

impl AlbumFs {
    pub fn open(dir: &std::path::Path) -> Result<Self, FsError> {
        let (sb, mut store) = open(dir)?;
        let bitmap = load_bitmap(&mut store, &sb)?;
        Ok(Self { store, sb, bitmap })
    }

    pub fn open_with_passphrase(
        dir: &std::path::Path,
        passphrase: Option<&str>,
    ) -> Result<Self, FsError> {
        let (sb, mut store) = open_with_passphrase(dir, passphrase)?;
        let bitmap = load_bitmap(&mut store, &sb)?;
        Ok(Self { store, sb, bitmap })
    }

    pub fn open_with_anchor(
        dir: &std::path::Path,
        anchor: &std::path::Path,
        passphrase: &str,
    ) -> Result<Self, FsError> {
        let (sb, mut store) = open_with_anchor(dir, anchor, passphrase)?;
        let bitmap = load_bitmap(&mut store, &sb)?;
        Ok(Self { store, sb, bitmap })
    }

    pub fn sync(&mut self) -> Result<(), FsError> {
        write_bitmap(&mut self.store, &self.sb, &self.bitmap)?;
        self.store.flush()
    }

    pub fn total_blocks(&self) -> u64 {
        self.sb.total_blocks
    }

    pub fn free_blocks(&self) -> u64 {
        self.bitmap.count_free()
    }

    pub fn inode_count(&self) -> u64 {
        self.sb.inode_count
    }

    pub fn used_inodes(&mut self) -> Result<u64, FsError> {
        let mut used = 0;
        for ino in 1..=self.sb.inode_count {
            if self.read_inode(ino)?.mode != 0 {
                used += 1;
            }
        }
        Ok(used)
    }

    pub fn read_inode(&mut self, ino: u64) -> Result<Inode, FsError> {
        let (block, offset) = self.inode_location(ino)?;
        let payload = self.store.read_block(block)?;
        Inode::decode(&payload[offset..offset + INODE_SIZE])
    }

    pub fn write_inode(&mut self, ino: u64, inode: &Inode) -> Result<(), FsError> {
        let (block, offset) = self.inode_location(ino)?;
        let mut payload = self.store.read_block(block)?;
        payload[offset..offset + INODE_SIZE].copy_from_slice(&inode.encode());
        self.store.write_block(block, &payload)
    }

    fn inode_location(&self, ino: u64) -> Result<(u64, usize), FsError> {
        if ino == 0 || ino > self.sb.inode_count {
            return Err(FsError::NotFound);
        }
        let byte_offset = (ino - 1) as usize * INODE_SIZE;
        Ok((
            self.sb.inode_table_start + (byte_offset / UB) as u64,
            byte_offset % UB,
        ))
    }

    pub fn alloc_inode(&mut self) -> Result<u64, FsError> {
        for ino in 1..=self.sb.inode_count {
            if self.read_inode(ino)?.mode == 0 {
                return Ok(ino);
            }
        }
        Err(FsError::NoSpace)
    }

    pub fn free_inode(&mut self, ino: u64) -> Result<(), FsError> {
        self.write_inode(ino, &Inode::default())
    }

    pub fn getattr(&mut self, ino: u64) -> Result<Inode, FsError> {
        let inode = self.read_inode(ino)?;
        if inode.mode == 0 {
            Err(FsError::NotFound)
        } else {
            Ok(inode)
        }
    }

    fn allocate_block(&mut self) -> Result<u64, FsError> {
        let block = self.bitmap.alloc_from(self.sb.data_start)?;
        self.store.write_block(block, &[])?;
        Ok(block)
    }

    fn pointer_at(block: &[u8], index: usize) -> u64 {
        let offset = index * 8;
        u64::from_le_bytes(block[offset..offset + 8].try_into().unwrap())
    }

    fn set_pointer(block: &mut [u8], index: usize, pointer: u64) {
        let offset = index * 8;
        block[offset..offset + 8].copy_from_slice(&pointer.to_le_bytes());
    }

    pub fn bmap(
        &mut self,
        inode: &mut Inode,
        index: u64,
        allocate: bool,
    ) -> Result<Option<u64>, FsError> {
        if index < 12 {
            let slot = &mut inode.direct[index as usize];
            if *slot == 0 && allocate {
                *slot = self.allocate_block()?;
                inode.block_count += 1;
            }
            return Ok((*slot != 0).then_some(*slot));
        }

        let mut relative = index - 12;
        if relative < POINTERS_PER_BLOCK {
            if inode.single_indirect == 0 {
                if !allocate {
                    return Ok(None);
                }
                inode.single_indirect = self.allocate_block()?;
            }
            let mut pointers = self.store.read_block(inode.single_indirect)?;
            let pointer = Self::pointer_at(&pointers, relative as usize);
            if pointer == 0 && allocate {
                let new_block = self.allocate_block()?;
                Self::set_pointer(&mut pointers, relative as usize, new_block);
                self.store.write_block(inode.single_indirect, &pointers)?;
                inode.block_count += 1;
                return Ok(Some(new_block));
            }
            return Ok((pointer != 0).then_some(pointer));
        }

        relative -= POINTERS_PER_BLOCK;
        if relative >= POINTERS_PER_BLOCK * POINTERS_PER_BLOCK {
            return Err(FsError::BadBlock(index));
        }
        if inode.double_indirect == 0 {
            if !allocate {
                return Ok(None);
            }
            inode.double_indirect = self.allocate_block()?;
        }
        let outer_index = (relative / POINTERS_PER_BLOCK) as usize;
        let inner_index = (relative % POINTERS_PER_BLOCK) as usize;
        let mut outer = self.store.read_block(inode.double_indirect)?;
        let mut inner_block = Self::pointer_at(&outer, outer_index);
        if inner_block == 0 {
            if !allocate {
                return Ok(None);
            }
            inner_block = self.allocate_block()?;
            Self::set_pointer(&mut outer, outer_index, inner_block);
            self.store.write_block(inode.double_indirect, &outer)?;
        }
        let mut inner = self.store.read_block(inner_block)?;
        let pointer = Self::pointer_at(&inner, inner_index);
        if pointer == 0 && allocate {
            let new_block = self.allocate_block()?;
            Self::set_pointer(&mut inner, inner_index, new_block);
            self.store.write_block(inner_block, &inner)?;
            inode.block_count += 1;
            return Ok(Some(new_block));
        }
        Ok((pointer != 0).then_some(pointer))
    }

    pub fn lookup(&mut self, parent_ino: u64, name: &str) -> Result<Option<u64>, FsError> {
        let parent = self.getattr(parent_ino)?;
        if !parent.is_dir() {
            return Err(FsError::NotDir);
        }
        for (_, entry) in self.directory_entries(&parent)? {
            if entry.inode != 0 && entry.name == name {
                return Ok(Some(entry.inode));
            }
        }
        Ok(None)
    }

    pub fn create(&mut self, parent_ino: u64, name: &str, mode: u32) -> Result<u64, FsError> {
        if self.lookup(parent_ino, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let ino = self.alloc_inode()?;
        let now = now();
        let inode = Inode {
            mode: S_IFREG | (mode & 0o7777),
            nlink: 1,
            atime: now,
            mtime: now,
            ctime: now,
            ..Inode::default()
        };
        self.write_inode(ino, &inode)?;
        self.add_dir_entry(parent_ino, ino, name, FILE_TYPE_FILE)?;
        self.sync()?;
        Ok(ino)
    }

    pub fn mkdir(&mut self, parent_ino: u64, name: &str, mode: u32) -> Result<u64, FsError> {
        if self.lookup(parent_ino, name)?.is_some() {
            return Err(FsError::Exists);
        }
        if !self.getattr(parent_ino)?.is_dir() {
            return Err(FsError::NotDir);
        }
        let ino = self.alloc_inode()?;
        let data_block = self.allocate_block()?;
        self.store.write_block(
            data_block,
            &super::dirent::initial_directory_block(ino, parent_ino)?,
        )?;
        let now = now();
        let mut inode = Inode {
            mode: S_IFDIR | (mode & 0o7777),
            nlink: 2,
            size: UB as u64,
            atime: now,
            mtime: now,
            ctime: now,
            block_count: 1,
            ..Inode::default()
        };
        inode.direct[0] = data_block;
        self.write_inode(ino, &inode)?;
        self.add_dir_entry(parent_ino, ino, name, FILE_TYPE_DIR)?;
        let mut parent = self.getattr(parent_ino)?;
        parent.nlink += 1;
        parent.mtime = now;
        parent.ctime = now;
        self.write_inode(parent_ino, &parent)?;
        self.sync()?;
        Ok(ino)
    }

    pub fn read(&mut self, ino: u64, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let mut inode = self.getattr(ino)?;
        if inode.is_dir() {
            return Err(FsError::IsDir);
        }
        if offset >= inode.size || len == 0 {
            return Ok(Vec::new());
        }
        let end = inode.size.min(offset.saturating_add(len as u64));
        let mut position = offset;
        let mut output = Vec::with_capacity((end - offset) as usize);
        while position < end {
            let logical = position / UB as u64;
            let within = (position % UB as u64) as usize;
            let take = ((end - position) as usize).min(UB - within);
            if let Some(block) = self.bmap(&mut inode, logical, false)? {
                let payload = self.store.read_block(block)?;
                output.extend_from_slice(&payload[within..within + take]);
            } else {
                output.resize(output.len() + take, 0);
            }
            position += take as u64;
        }
        Ok(output)
    }

    pub fn write(&mut self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let mut inode = self.getattr(ino)?;
        if inode.is_dir() {
            return Err(FsError::IsDir);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or_else(|| FsError::Manifest("file offset overflow".into()))?;
        let mut position = offset;
        let mut consumed = 0usize;
        while consumed < data.len() {
            let logical = position / UB as u64;
            let within = (position % UB as u64) as usize;
            let take = (data.len() - consumed).min(UB - within);
            let block = self
                .bmap(&mut inode, logical, true)?
                .ok_or(FsError::NoSpace)?;
            let mut payload = self.store.read_block(block)?;
            payload[within..within + take].copy_from_slice(&data[consumed..consumed + take]);
            self.store.write_block(block, &payload)?;
            position += take as u64;
            consumed += take;
        }
        if end > inode.size {
            inode.size = end;
        }
        let timestamp = now();
        inode.mtime = timestamp;
        inode.ctime = timestamp;
        self.write_inode(ino, &inode)?;
        self.sync()?;
        Ok(consumed)
    }

    pub fn readdir(&mut self, ino: u64) -> Result<Vec<(u64, String, u8)>, FsError> {
        let inode = self.getattr(ino)?;
        if !inode.is_dir() {
            return Err(FsError::NotDir);
        }
        Ok(self
            .directory_entries(&inode)?
            .into_iter()
            .filter(|(_, entry)| entry.inode != 0)
            .map(|(_, entry)| (entry.inode, entry.name, entry.file_type))
            .collect())
    }

    pub fn unlink(&mut self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let ino = self.lookup(parent_ino, name)?.ok_or(FsError::NotFound)?;
        let mut inode = self.getattr(ino)?;
        if inode.is_dir() {
            return Err(FsError::IsDir);
        }
        self.remove_dir_entry(parent_ino, name)?;
        inode.nlink = inode.nlink.saturating_sub(1);
        if inode.nlink == 0 {
            self.free_all_blocks(&mut inode)?;
            self.free_inode(ino)?;
        } else {
            inode.ctime = now();
            self.write_inode(ino, &inode)?;
        }
        self.sync()
    }

    pub fn rmdir(&mut self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let ino = self.lookup(parent_ino, name)?.ok_or(FsError::NotFound)?;
        let mut inode = self.getattr(ino)?;
        if !inode.is_dir() {
            return Err(FsError::NotDir);
        }
        if self
            .readdir(ino)?
            .iter()
            .any(|(_, entry_name, _)| entry_name != "." && entry_name != "..")
        {
            return Err(FsError::NotEmpty);
        }
        self.remove_dir_entry(parent_ino, name)?;
        self.free_all_blocks(&mut inode)?;
        self.free_inode(ino)?;
        let mut parent = self.getattr(parent_ino)?;
        parent.nlink = parent.nlink.saturating_sub(1);
        parent.mtime = now();
        parent.ctime = parent.mtime;
        self.write_inode(parent_ino, &parent)?;
        self.sync()
    }

    pub fn rename(
        &mut self,
        parent_ino: u64,
        name: &str,
        new_parent_ino: u64,
        new_name: &str,
    ) -> Result<(), FsError> {
        if parent_ino == new_parent_ino && name == new_name {
            return Ok(());
        }
        let ino = self.lookup(parent_ino, name)?.ok_or(FsError::NotFound)?;
        let source = self.getattr(ino)?;
        if let Some(target_ino) = self.lookup(new_parent_ino, new_name)? {
            if target_ino == ino {
                return Ok(());
            }
            let target = self.getattr(target_ino)?;
            match (source.is_dir(), target.is_dir()) {
                (true, false) => return Err(FsError::NotDir),
                (false, true) => return Err(FsError::IsDir),
                (true, true) => self.rmdir(new_parent_ino, new_name)?,
                (false, false) => self.unlink(new_parent_ino, new_name)?,
            }
        }

        let file_type = if source.is_dir() {
            FILE_TYPE_DIR
        } else {
            FILE_TYPE_FILE
        };
        self.add_dir_entry(new_parent_ino, ino, new_name, file_type)?;
        self.remove_dir_entry(parent_ino, name)?;

        if source.is_dir() && parent_ino != new_parent_ino {
            self.update_dotdot(&source, new_parent_ino)?;
            let timestamp = now();
            let mut old_parent = self.getattr(parent_ino)?;
            old_parent.nlink = old_parent.nlink.saturating_sub(1);
            old_parent.mtime = timestamp;
            old_parent.ctime = timestamp;
            self.write_inode(parent_ino, &old_parent)?;
            let mut new_parent = self.getattr(new_parent_ino)?;
            new_parent.nlink += 1;
            new_parent.mtime = timestamp;
            new_parent.ctime = timestamp;
            self.write_inode(new_parent_ino, &new_parent)?;
        }
        self.sync()
    }

    pub fn truncate(&mut self, ino: u64, new_size: u64) -> Result<(), FsError> {
        let mut inode = self.getattr(ino)?;
        if inode.is_dir() {
            return Err(FsError::IsDir);
        }
        if new_size < inode.size {
            let keep_blocks = new_size.div_ceil(UB as u64);
            let old_blocks = inode.size.div_ceil(UB as u64);
            for index in keep_blocks..old_blocks {
                self.remove_mapped_block(&mut inode, index)?;
            }
            let tail = (new_size % UB as u64) as usize;
            if tail != 0 {
                if let Some(block) = self.bmap(&mut inode, keep_blocks - 1, false)? {
                    let mut payload = self.store.read_block(block)?;
                    payload[tail..].fill(0);
                    self.store.write_block(block, &payload)?;
                }
            }
        }
        inode.size = new_size;
        inode.mtime = now();
        inode.ctime = inode.mtime;
        self.write_inode(ino, &inode)?;
        self.sync()
    }

    fn directory_entries(&mut self, inode: &Inode) -> Result<Vec<(u64, DirEntry)>, FsError> {
        let mut entries = Vec::new();
        let blocks = inode.size.div_ceil(UB as u64);
        let mut inode_copy = inode.clone();
        for logical in 0..blocks {
            let Some(block) = self.bmap(&mut inode_copy, logical, false)? else {
                continue;
            };
            let payload = self.store.read_block(block)?;
            let mut offset = 0usize;
            while offset < UB {
                let entry = DirEntry::decode(&payload, offset)?;
                let rec_len = entry.rec_len as usize;
                entries.push((logical * UB as u64 + offset as u64, entry));
                offset += rec_len;
            }
        }
        Ok(entries)
    }

    fn add_dir_entry(
        &mut self,
        parent_ino: u64,
        ino: u64,
        name: &str,
        file_type: u8,
    ) -> Result<(), FsError> {
        let mut parent = self.getattr(parent_ino)?;
        if !parent.is_dir() {
            return Err(FsError::NotDir);
        }
        let needed = DirEntry::required_len(name.len());
        if needed > UB || name.len() > u8::MAX as usize {
            return Err(FsError::NoSpace);
        }
        let block_count = parent.size.div_ceil(UB as u64);
        for logical in 0..block_count {
            let block = self
                .bmap(&mut parent, logical, false)?
                .ok_or_else(|| FsError::Manifest("directory has a hole".into()))?;
            let mut payload = self.store.read_block(block)?;
            let mut offset = 0usize;
            while offset < UB {
                let entry = DirEntry::decode(&payload, offset)?;
                let rec_len = entry.rec_len as usize;
                if entry.inode == 0 && rec_len >= needed {
                    DirEntry::new(ino, name, file_type, rec_len)?
                        .encode_into(&mut payload[offset..offset + rec_len])?;
                    self.store.write_block(block, &payload)?;
                    self.bump_directory(parent_ino, parent)?;
                    return Ok(());
                }
                let minimal = DirEntry::required_len(entry.name.len());
                if entry.inode != 0 && rec_len - minimal >= needed {
                    payload[offset + 8..offset + 10]
                        .copy_from_slice(&(minimal as u16).to_le_bytes());
                    let new_offset = offset + minimal;
                    let new_len = rec_len - minimal;
                    DirEntry::new(ino, name, file_type, new_len)?
                        .encode_into(&mut payload[new_offset..new_offset + new_len])?;
                    self.store.write_block(block, &payload)?;
                    self.bump_directory(parent_ino, parent)?;
                    return Ok(());
                }
                offset += rec_len;
            }
        }

        let logical = block_count;
        let block = self
            .bmap(&mut parent, logical, true)?
            .ok_or(FsError::NoSpace)?;
        let mut payload = [0u8; UB];
        DirEntry::new(ino, name, file_type, UB)?.encode_into(&mut payload)?;
        self.store.write_block(block, &payload)?;
        parent.size += UB as u64;
        self.bump_directory(parent_ino, parent)
    }

    fn bump_directory(&mut self, ino: u64, mut inode: Inode) -> Result<(), FsError> {
        inode.mtime = now();
        inode.ctime = inode.mtime;
        self.write_inode(ino, &inode)
    }

    fn remove_dir_entry(&mut self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let mut parent = self.getattr(parent_ino)?;
        if !parent.is_dir() {
            return Err(FsError::NotDir);
        }
        let blocks = parent.size.div_ceil(UB as u64);
        for logical in 0..blocks {
            let block = self
                .bmap(&mut parent, logical, false)?
                .ok_or_else(|| FsError::Manifest("directory has a hole".into()))?;
            let mut payload = self.store.read_block(block)?;
            let mut offset = 0usize;
            let mut previous = None;
            while offset < UB {
                let entry = DirEntry::decode(&payload, offset)?;
                let rec_len = entry.rec_len as usize;
                if entry.inode != 0 && entry.name == name {
                    if let Some(previous_offset) = previous {
                        let old_len = u16::from_le_bytes(
                            payload[previous_offset + 8..previous_offset + 10]
                                .try_into()
                                .unwrap(),
                        ) as usize;
                        let combined = old_len + rec_len;
                        payload[previous_offset + 8..previous_offset + 10]
                            .copy_from_slice(&(combined as u16).to_le_bytes());
                    } else {
                        payload[offset..offset + 8].fill(0);
                    }
                    self.store.write_block(block, &payload)?;
                    self.bump_directory(parent_ino, parent)?;
                    return Ok(());
                }
                if entry.inode != 0 {
                    previous = Some(offset);
                }
                offset += rec_len;
            }
        }
        Err(FsError::NotFound)
    }

    fn update_dotdot(&mut self, directory: &Inode, new_parent: u64) -> Result<(), FsError> {
        let mut directory_copy = directory.clone();
        let block = self
            .bmap(&mut directory_copy, 0, false)?
            .ok_or_else(|| FsError::Manifest("directory has no first block".into()))?;
        let mut payload = self.store.read_block(block)?;
        let mut offset = 0usize;
        while offset < UB {
            let entry = DirEntry::decode(&payload, offset)?;
            if entry.inode != 0 && entry.name == ".." {
                payload[offset..offset + 8].copy_from_slice(&new_parent.to_le_bytes());
                self.store.write_block(block, &payload)?;
                return Ok(());
            }
            offset += entry.rec_len as usize;
        }
        Err(FsError::Manifest("directory has no parent entry".into()))
    }

    fn remove_mapped_block(&mut self, inode: &mut Inode, index: u64) -> Result<(), FsError> {
        if index < 12 {
            let block = std::mem::take(&mut inode.direct[index as usize]);
            if block != 0 {
                self.bitmap.clear(block);
                inode.block_count -= 1;
            }
            return Ok(());
        }
        let mut relative = index - 12;
        if relative < POINTERS_PER_BLOCK {
            if inode.single_indirect == 0 {
                return Ok(());
            }
            let mut pointers = self.store.read_block(inode.single_indirect)?;
            let block = Self::pointer_at(&pointers, relative as usize);
            if block != 0 {
                self.bitmap.clear(block);
                inode.block_count -= 1;
                Self::set_pointer(&mut pointers, relative as usize, 0);
            }
            if pointers.iter().all(|byte| *byte == 0) {
                self.bitmap.clear(inode.single_indirect);
                inode.single_indirect = 0;
            } else {
                self.store.write_block(inode.single_indirect, &pointers)?;
            }
            return Ok(());
        }
        relative -= POINTERS_PER_BLOCK;
        if relative >= POINTERS_PER_BLOCK * POINTERS_PER_BLOCK || inode.double_indirect == 0 {
            return Ok(());
        }
        let outer_index = (relative / POINTERS_PER_BLOCK) as usize;
        let inner_index = (relative % POINTERS_PER_BLOCK) as usize;
        let mut outer = self.store.read_block(inode.double_indirect)?;
        let inner_block = Self::pointer_at(&outer, outer_index);
        if inner_block == 0 {
            return Ok(());
        }
        let mut inner = self.store.read_block(inner_block)?;
        let block = Self::pointer_at(&inner, inner_index);
        if block != 0 {
            self.bitmap.clear(block);
            inode.block_count -= 1;
            Self::set_pointer(&mut inner, inner_index, 0);
        }
        if inner.iter().all(|byte| *byte == 0) {
            self.bitmap.clear(inner_block);
            Self::set_pointer(&mut outer, outer_index, 0);
        } else {
            self.store.write_block(inner_block, &inner)?;
        }
        if outer.iter().all(|byte| *byte == 0) {
            self.bitmap.clear(inode.double_indirect);
            inode.double_indirect = 0;
        } else {
            self.store.write_block(inode.double_indirect, &outer)?;
        }
        Ok(())
    }

    fn free_all_blocks(&mut self, inode: &mut Inode) -> Result<(), FsError> {
        let logical_blocks = inode.size.div_ceil(UB as u64);
        for index in 0..logical_blocks {
            self.remove_mapped_block(inode, index)?;
        }
        inode.size = 0;
        Ok(())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
