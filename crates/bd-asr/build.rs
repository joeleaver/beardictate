//! sherpa-onnx links as shared libraries (see the workspace Cargo.toml) and
//! its build script copies them into the profile directory. Tests
//! run from `deps/` below it.

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let origin = match os.as_str() {
        "linux" => "$ORIGIN",
        "macos" => "@loader_path",
        _ => return,
    };
    println!("cargo:rustc-link-arg-tests=-Wl,-rpath,{origin}/..");
}
