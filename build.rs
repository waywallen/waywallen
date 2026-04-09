//! Build script for waywallen.
//!
//! Generates `display_proto_generated.rs` from the
//! `waywallen-display-v1` XML description into `$OUT_DIR`, where
//! `src/display_proto/mod.rs` pulls it in via `include!`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let xml_path = manifest_dir.join("protocol/waywallen_display_v1.xml");

    // Rerun triggers: the XML itself and the codegen tool sources.
    println!("cargo:rerun-if-changed={}", xml_path.display());
    let tool_src = manifest_dir.join("tools/wayproto-gen/src");
    for name in ["lib.rs", "parser.rs", "codegen_rust.rs", "main.rs"] {
        println!("cargo:rerun-if-changed={}", tool_src.join(name).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("tools/wayproto-gen/Cargo.toml").display()
    );

    let xml = fs::read_to_string(&xml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", xml_path.display()));
    let code = wayproto_gen::emit_rust_from_xml(&xml)
        .unwrap_or_else(|e| panic!("wayproto-gen failed: {e}"));

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_file = out_dir.join("display_proto_generated.rs");
    fs::write(&out_file, code)
        .unwrap_or_else(|e| panic!("write {}: {e}", out_file.display()));
}
