use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=TOOLCHAINS");
    println!("cargo:rerun-if-env-changed=INDEXSTORE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PATH");

    let library_dir = env::var_os("INDEXSTORE_LIB_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let output = if cfg!(target_os = "macos") {
            Command::new("xcrun").args(["--find", "swiftc"]).output()
        } else {
            Command::new("sh").args(["-c", "command -v swiftc"]).output()
        }
        .expect("Install the Swift toolchain or set INDEXSTORE_LIB_DIR");
        assert!(output.status.success(), "Could not locate swiftc. Set INDEXSTORE_LIB_DIR to the directory containing libIndexStore");
        let swiftc = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).canonicalize().expect("resolve swiftc");
        swiftc.parent().unwrap().parent().unwrap().join("lib")
    });

    println!("cargo:rustc-link-search=native={}", library_dir.display());
    println!("cargo:rustc-link-lib=dylib=IndexStore");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", library_dir.display());
}
