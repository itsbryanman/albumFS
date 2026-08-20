use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/stego/jpeg_bridge.c");

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo did not set OUT_DIR"));
    let object = output.join("albumfs_jpeg_bridge.o");
    let archive = output.join("libalbumfs_jpeg_bridge.a");
    let includes = env::var_os("DEP_JPEG_INCLUDE")
        .expect("mozjpeg-sys did not publish its include directories");

    let mut compiler = Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()));
    compiler
        .arg("-std=c11")
        .arg("-fPIC")
        .arg("-c")
        .arg("src/stego/jpeg_bridge.c")
        .arg("-o")
        .arg(&object);
    for include in env::split_paths(&includes) {
        compiler.arg("-I").arg(include);
    }
    let status = compiler.status().expect("failed to start the C compiler");
    assert!(status.success(), "failed to compile the JPEG error bridge");

    let status = Command::new(env::var_os("AR").unwrap_or_else(|| "ar".into()))
        .arg("crs")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("failed to start the native archiver");
    assert!(status.success(), "failed to archive the JPEG error bridge");

    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=albumfs_jpeg_bridge");
}
