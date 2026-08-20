use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};

use super::inode::S_IFMT;
use super::{AlbumFs, FsError, Inode};

const TTL: Duration = Duration::from_secs(1);

pub fn errno_for(error: &FsError) -> i32 {
    match error {
        FsError::NotFound => libc::ENOENT,
        FsError::Exists => libc::EEXIST,
        FsError::NotEmpty => libc::ENOTEMPTY,
        FsError::NotDir => libc::ENOTDIR,
        FsError::IsDir => libc::EISDIR,
        FsError::NoSpace => libc::ENOSPC,
        FsError::Auth => libc::EACCES,
        _ => libc::EIO,
    }
}

pub fn to_fileattr(ino: u64, inode: &Inode) -> FileAttr {
    let atime = timestamp(inode.atime);
    let mtime = timestamp(inode.mtime);
    let ctime = timestamp(inode.ctime);
    FileAttr {
        ino,
        size: inode.size,
        blocks: inode.size.div_ceil(512),
        atime,
        mtime,
        ctime,
        crtime: ctime,
        kind: if inode.is_dir() {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        perm: (inode.mode & 0o7777) as u16,
        nlink: inode.nlink,
        uid: inode.uid,
        gid: inode.gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

pub fn readdir_page(entry_count: usize, offset: i64, max_entries: usize) -> Vec<(usize, i64)> {
    let start = usize::try_from(offset.max(0)).unwrap_or(usize::MAX);
    (start..entry_count)
        .take(max_entries)
        .map(|index| (index, (index + 1) as i64))
        .collect()
}

pub struct FuseAlbumFs {
    fs: Arc<Mutex<AlbumFs>>,
}

impl FuseAlbumFs {
    pub fn new(fs: Arc<Mutex<AlbumFs>>) -> Self {
        Self { fs }
    }

    fn lock(&self) -> Result<MutexGuard<'_, AlbumFs>, i32> {
        self.fs.lock().map_err(|_| libc::EIO)
    }

    fn sync_reply(&self, reply: ReplyEmpty) {
        match self
            .lock()
            .and_then(|mut fs| fs.sync().map_err(|_| libc::EIO))
        {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }
}

impl Filesystem for FuseAlbumFs {
    // AlbumFS has no file handles, so open and opendir keep the fuser defaults.

    fn destroy(&mut self) {
        if let Ok(mut fs) = self.lock() {
            let _ = fs.sync();
        }
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        let result = self.lock().and_then(|mut fs| {
            let ino = fs
                .lookup(parent, name)
                .map_err(|error| errno_for(&error))?
                .ok_or(libc::ENOENT)?;
            let inode = fs.getattr(ino).map_err(|error| errno_for(&error))?;
            Ok((ino, inode))
        });
        match result {
            Ok((ino, inode)) => reply.entry(&TTL, &to_fileattr(ino, &inode), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        match self
            .lock()
            .and_then(|mut fs| fs.getattr(ino).map_err(|error| errno_for(&error)))
        {
            Ok(inode) => reply.attr(&TTL, &to_fileattr(ino, &inode)),
            Err(errno) => reply.error(errno),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let result = self.lock().and_then(|mut fs| {
            if let Some(new_size) = size {
                fs.truncate(ino, new_size)
                    .map_err(|error| errno_for(&error))?;
            }
            let mut inode = fs.getattr(ino).map_err(|error| errno_for(&error))?;
            if let Some(new_mode) = mode {
                inode.mode = (inode.mode & S_IFMT) | (new_mode & 0o7777);
            }
            if let Some(new_uid) = uid {
                inode.uid = new_uid;
            }
            if let Some(new_gid) = gid {
                inode.gid = new_gid;
            }
            if let Some(new_atime) = atime {
                inode.atime = time_or_now_seconds(new_atime);
            }
            if let Some(new_mtime) = mtime {
                inode.mtime = time_or_now_seconds(new_mtime);
            }
            if let Some(new_ctime) = ctime {
                inode.ctime = system_time_seconds(new_ctime);
            }
            fs.write_inode(ino, &inode)
                .and_then(|_| fs.sync())
                .map_err(|error| errno_for(&error))?;
            Ok(inode)
        });
        match result {
            Ok(inode) => reply.attr(&TTL, &to_fileattr(ino, &inode)),
            Err(errno) => reply.error(errno),
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        match self.lock().and_then(|mut fs| {
            fs.read(ino, offset as u64, size as usize)
                .map_err(|error| errno_for(&error))
        }) {
            Ok(bytes) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        match self.lock().and_then(|mut fs| {
            fs.write(ino, offset as u64, data)
                .map_err(|error| errno_for(&error))
        }) {
            Ok(written) => reply.written(written as u32),
            Err(errno) => reply.error(errno),
        }
    }

    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        let result = self.lock().and_then(|mut fs| {
            let ino = fs
                .create(parent, name, mode & !umask)
                .map_err(|error| errno_for(&error))?;
            let mut inode = fs.getattr(ino).map_err(|error| errno_for(&error))?;
            inode.uid = req.uid();
            inode.gid = req.gid();
            fs.write_inode(ino, &inode)
                .and_then(|_| fs.sync())
                .map_err(|error| errno_for(&error))?;
            Ok((ino, inode))
        });
        match result {
            Ok((ino, inode)) => reply.created(&TTL, &to_fileattr(ino, &inode), 0, 0, 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        let result = self.lock().and_then(|mut fs| {
            let ino = fs
                .mkdir(parent, name, mode & !umask)
                .map_err(|error| errno_for(&error))?;
            let mut inode = fs.getattr(ino).map_err(|error| errno_for(&error))?;
            inode.uid = req.uid();
            inode.gid = req.gid();
            fs.write_inode(ino, &inode)
                .and_then(|_| fs.sync())
                .map_err(|error| errno_for(&error))?;
            Ok((ino, inode))
        });
        match result {
            Ok((ino, inode)) => reply.entry(&TTL, &to_fileattr(ino, &inode), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        match self
            .lock()
            .and_then(|mut fs| fs.unlink(parent, name).map_err(|error| errno_for(&error)))
        {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let Some(name) = name.to_str() else {
            reply.error(libc::EINVAL);
            return;
        };
        match self
            .lock()
            .and_then(|mut fs| fs.rmdir(parent, name).map_err(|error| errno_for(&error)))
        {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        if flags != 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let (Some(name), Some(newname)) = (name.to_str(), newname.to_str()) else {
            reply.error(libc::EINVAL);
            return;
        };
        match self.lock().and_then(|mut fs| {
            fs.rename(parent, name, newparent, newname)
                .map_err(|error| errno_for(&error))
        }) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let entries = match self
            .lock()
            .and_then(|mut fs| fs.readdir(ino).map_err(|error| errno_for(&error)))
        {
            Ok(entries) => entries,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        for (index, next_offset) in readdir_page(entries.len(), offset, entries.len()) {
            let (entry_ino, name, file_type) = &entries[index];
            let kind = if *file_type == 2 {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            if reply.add(*entry_ino, next_offset, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        self.sync_reply(reply);
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.sync_reply(reply);
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        self.sync_reply(reply);
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.sync_reply(reply);
    }

    fn fsyncdir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.sync_reply(reply);
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        let stats = self.lock().and_then(|mut fs| {
            let used_inodes = fs.used_inodes().map_err(|error| errno_for(&error))?;
            Ok((
                fs.total_blocks(),
                fs.free_blocks(),
                fs.inode_count(),
                used_inodes,
            ))
        });
        match stats {
            Ok((blocks, free, inodes, used_inodes)) => reply.statfs(
                blocks,
                free,
                free,
                inodes,
                inodes - used_inodes,
                4096,
                255,
                4096,
            ),
            Err(errno) => reply.error(errno),
        }
    }
}

pub fn mount(dir: &Path, mountpoint: &Path) -> Result<(), FsError> {
    mount_with_passphrase(dir, mountpoint, None)
}

pub fn mount_with_passphrase(
    dir: &Path,
    mountpoint: &Path,
    passphrase: Option<&str>,
) -> Result<(), FsError> {
    let fs = AlbumFs::open_with_passphrase(dir, passphrase)?;
    mount_filesystem(fs, mountpoint)
}

pub fn mount_with_anchor(
    dir: &Path,
    mountpoint: &Path,
    anchor: &Path,
    passphrase: &str,
) -> Result<(), FsError> {
    let fs = AlbumFs::open_with_anchor(dir, anchor, passphrase)?;
    mount_filesystem(fs, mountpoint)
}

fn mount_filesystem(mut fs: AlbumFs, mountpoint: &Path) -> Result<(), FsError> {
    fs.sync()?;
    let shared = Arc::new(Mutex::new(fs));
    let adapter = FuseAlbumFs::new(Arc::clone(&shared));
    let options = [
        MountOption::FSName("albumfs".into()),
        MountOption::AutoUnmount,
        MountOption::DefaultPermissions,
    ];
    let mount_result = fuser::mount2(adapter, mountpoint, &options);
    let sync_result = lock_shared(&shared).and_then(|mut fs| fs.sync());
    mount_result.map_err(FsError::Io)?;
    sync_result
}

pub fn umount(mountpoint: &Path) -> Result<(), FsError> {
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("fusermount")
        .arg("-u")
        .arg(mountpoint)
        .status()?;
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("umount")
        .arg(mountpoint)
        .status()?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err(FsError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "umount is supported only on Linux and macOS",
    )));

    if status.success() {
        Ok(())
    } else {
        Err(FsError::Io(io::Error::other(format!(
            "unmount command exited with {status}"
        ))))
    }
}

fn lock_shared(shared: &Arc<Mutex<AlbumFs>>) -> Result<MutexGuard<'_, AlbumFs>, FsError> {
    shared
        .lock()
        .map_err(|_| FsError::Io(io::Error::other("filesystem lock poisoned")))
}

fn timestamp(seconds: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH)
}

fn system_time_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn time_or_now_seconds(time: TimeOrNow) -> u64 {
    match time {
        TimeOrNow::SpecificTime(time) => system_time_seconds(time),
        TimeOrNow::Now => system_time_seconds(SystemTime::now()),
    }
}
