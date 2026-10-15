//! sherpa-onnx links as shared libraries (see the workspace Cargo.toml) and
//! its build script copies them next to the binary. Point the loader there.

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "linux" {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    } else if os == "macos" {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@loader_path");
    }
}
