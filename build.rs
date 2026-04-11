//! Build script for waywallen.
//!
//! Generates Rust bindings for both wire protocols from XML into
//! `$OUT_DIR`:
//!   - `display_proto_generated.rs`  (src/display_proto/mod.rs includes it)
//!   - `ipc_generated.rs`            (src/ipc/mod.rs includes it)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Rerun triggers: the codegen tool sources (XMLs are added per file).
    let tool_src = manifest_dir.join("tools/wayproto-gen/src");
    for name in ["lib.rs", "parser.rs", "codegen_rust.rs", "codegen_c.rs", "main.rs"] {
        println!("cargo:rerun-if-changed={}", tool_src.join(name).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("tools/wayproto-gen/Cargo.toml").display()
    );

    gen_rust(
        &manifest_dir.join("protocol/waywallen_display_v1.xml"),
        &out_dir.join("display_proto_generated.rs"),
    );
    gen_rust(
        &manifest_dir.join("protocol/waywallen_ipc_v1.xml"),
        &out_dir.join("ipc_generated.rs"),
    );

    // Control plane protobufs (prost).
    let proto_path = manifest_dir.join("proto/control.proto");
    println!("cargo:rerun-if-changed={}", proto_path.display());
    prost_build::Config::new()
        .compile_protos(&[proto_path], &[manifest_dir.join("proto")])
        .expect("prost-build failed on proto/control.proto");
}

fn gen_rust(xml_path: &Path, out_file: &Path) {
    println!("cargo:rerun-if-changed={}", xml_path.display());
    let xml = fs::read_to_string(xml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", xml_path.display()));
    let code = wayproto_gen::emit_rust_from_xml(&xml)
        .unwrap_or_else(|e| panic!("wayproto-gen failed on {}: {e}", xml_path.display()));
    fs::write(out_file, code)
        .unwrap_or_else(|e| panic!("write {}: {e}", out_file.display()));
}
