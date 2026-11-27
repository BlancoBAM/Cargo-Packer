use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let profile = env::var("PROFILE").unwrap_or_default();
    let out_dir = env::var("OUT_DIR").unwrap_or_default();

    if target != "wasm32-unknown-unknown" || profile != "release" {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let target_dir = PathBuf::from(&out_dir)
        .ancestors()
        .nth(3)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(&manifest_dir).join("target"));

    let wasm_name = env::var("CARGO_CRATE_NAME").unwrap_or_default().replace('-', "_");
    let wasm_path = target_dir
        .join("wasm32-unknown-unknown")
        .join("release")
        .join(format!("{}.wasm", wasm_name));

    if !wasm_path.exists() {
        return;
    }

    let status = Command::new("wasm-opt")
        .args(["-Oz", "--enable-mutable-globals"])
        .arg(&wasm_path)
        .arg("-o")
        .arg(&wasm_path)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=wasm-opt optimization applied to {}", wasm_path.display());
        }
        Ok(s) => {
            println!("cargo:warning=wasm-opt failed with exit code: {}", s);
        }
        Err(e) => {
            println!("cargo:warning=wasm-opt not found, skipping optimization: {}", e);
        }
    }
}