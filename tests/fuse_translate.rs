use albumfs::fs::fuse::{errno_for, readdir_page, to_fileattr};
use albumfs::fs::inode::{S_IFDIR, S_IFREG};
use albumfs::fs::{FsError, Inode};
use fuser::FileType;

#[test]
fn attr_translation() {
    let directory = Inode {
        mode: S_IFDIR | 0o751,
        uid: 1000,
        gid: 1001,
        nlink: 3,
        size: 4032,
        atime: 10,
        mtime: 20,
        ctime: 30,
        ..Inode::default()
    };
    let attr = to_fileattr(7, &directory);
    assert_eq!(attr.ino, 7);
    assert_eq!(attr.kind, FileType::Directory);
    assert_eq!(attr.perm, 0o751);
    assert_eq!(attr.size, 4032);
    assert_eq!(attr.nlink, 3);
    assert_eq!(attr.uid, 1000);
    assert_eq!(attr.gid, 1001);

    let file = Inode {
        mode: S_IFREG | 0o640,
        nlink: 1,
        size: 513,
        ..Inode::default()
    };
    let attr = to_fileattr(8, &file);
    assert_eq!(attr.kind, FileType::RegularFile);
    assert_eq!(attr.perm, 0o640);
    assert_eq!(attr.size, 513);
    assert_eq!(attr.blocks, 2);
    assert_eq!(attr.nlink, 1);
}

#[test]
fn errno_mapping() {
    assert_eq!(errno_for(&FsError::NotFound), libc::ENOENT);
    assert_eq!(errno_for(&FsError::Exists), libc::EEXIST);
    assert_eq!(errno_for(&FsError::NotEmpty), libc::ENOTEMPTY);
    assert_eq!(errno_for(&FsError::NotDir), libc::ENOTDIR);
    assert_eq!(errno_for(&FsError::IsDir), libc::EISDIR);
    assert_eq!(errno_for(&FsError::NoSpace), libc::ENOSPC);
    assert_eq!(errno_for(&FsError::Auth), libc::EACCES);
    assert_eq!(errno_for(&FsError::BlockCrc(4)), libc::EIO);
}

#[test]
fn readdir_offset_paging() {
    let mut visited = Vec::new();
    let mut offset = 0;
    loop {
        let page = readdir_page(11, offset, 3);
        if page.is_empty() {
            break;
        }
        visited.extend(page.iter().map(|(index, _)| *index));
        offset = page.last().unwrap().1;
    }
    assert_eq!(visited, (0..11).collect::<Vec<_>>());
    assert_eq!(readdir_page(4, -1, 2), vec![(0, 1), (1, 2)]);
    assert!(readdir_page(4, 4, 2).is_empty());
}
