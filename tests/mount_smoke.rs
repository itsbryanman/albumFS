use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use albumfs::fs::format::format;
use image::{Rgba, RgbaImage};

fn make_pool(dir: &Path) {
    for image_index in 0..3 {
        let mut image = RgbaImage::new(768, 768);
        for (pixel_index, pixel) in image.pixels_mut().enumerate() {
            let value = (pixel_index as u32)
                .wrapping_mul(47)
                .wrapping_add(image_index * 89);
            *pixel = Rgba([value as u8, (value >> 6) as u8, (value >> 14) as u8, 255]);
        }
        image
            .save(dir.join(format!("carrier-{image_index:03}.png")))
            .unwrap();
    }
}

fn wait_for_mount(child: &mut Child, mountpoint: &Path) {
    let needle = mountpoint.to_string_lossy();
    for _ in 0..100 {
        if fs::read_to_string("/proc/self/mountinfo")
            .unwrap_or_default()
            .contains(needle.as_ref())
        {
            return;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("mount process exited early with {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("mount did not become ready");
}

fn start_mount(pool: &Path, mountpoint: &Path) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_albumfs"))
        .arg("mount")
        .arg(pool)
        .arg(mountpoint)
        .spawn()
        .unwrap();
    wait_for_mount(&mut child, mountpoint);
    child
}

fn stop_mount(child: &mut Child, mountpoint: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_albumfs"))
        .arg("umount")
        .arg(mountpoint)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(child.wait().unwrap().success());
}

#[test]
fn mount_roundtrip_and_remount() {
    if std::env::var("ALBUMFS_MOUNT_TESTS").as_deref() != Ok("1") {
        eprintln!("mount smoke test skipped: set ALBUMFS_MOUNT_TESTS=1 to enable it");
        return;
    }

    let pool = tempfile::tempdir().unwrap();
    let mountpoint = tempfile::tempdir().unwrap();
    make_pool(pool.path());
    format(pool.path()).unwrap();

    let mut session = start_mount(pool.path(), mountpoint.path());
    fs::create_dir(mountpoint.path().join("subdir")).unwrap();
    fs::write(mountpoint.path().join("message.txt"), b"hello through fuse").unwrap();
    assert_eq!(
        fs::read(mountpoint.path().join("message.txt")).unwrap(),
        b"hello through fuse"
    );
    let names: Vec<_> = fs::read_dir(mountpoint.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(names.iter().any(|name| name == "subdir"));
    assert!(names.iter().any(|name| name == "message.txt"));
    stop_mount(&mut session, mountpoint.path());

    let mut session = start_mount(pool.path(), mountpoint.path());
    assert!(mountpoint.path().join("subdir").is_dir());
    assert_eq!(
        fs::read(mountpoint.path().join("message.txt")).unwrap(),
        b"hello through fuse"
    );
    stop_mount(&mut session, mountpoint.path());
}
