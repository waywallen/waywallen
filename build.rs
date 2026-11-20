#[path = "tools/tray_i18n.rs"]
mod tray_i18n;

use tray_i18n::{find_ts_context, parse_ts_messages};

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Rerun triggers: the codegen tool sources (XMLs are added per file).
    let tool_src = manifest_dir.join("tools/wayproto-gen/src");
    for name in [
        "lib.rs",
        "parser.rs",
        "codegen_rust.rs",
        "codegen_c.rs",
        "main.rs",
    ] {
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
        &manifest_dir.join("protocol/waywallen_ipc_v3.xml"),
        &out_dir.join("ipc_generated.rs"),
    );

    // Control plane protobufs (prost).
    let proto_paths = [
        manifest_dir.join("proto/control.proto"),
        manifest_dir.join("proto/filter.proto"),
    ];
    for proto_path in &proto_paths {
        println!("cargo:rerun-if-changed={}", proto_path.display());
    }
    prost_build::Config::new()
        .compile_protos(&proto_paths, &[manifest_dir.join("proto")])
        .expect("prost-build failed on control/filter protos");

    gen_tray_i18n(
        &manifest_dir.join("ui/i18n"),
        &out_dir.join("tray_i18n_generated.rs"),
    );

    build_pulse_adapter(&manifest_dir);
    let snapshot = manifest_dir.join("src/wallframe/snapshot.c");
    println!("cargo:rerun-if-changed={}", snapshot.display());
    let mut build = cc::Build::new();
    build.file(snapshot).flag_if_supported("-std=gnu11");
    for package in ["egl", "glesv2"] {
        let library = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe(package)
            .expect("EGL and GLES2 development headers are required for wallpaper snapshots");
        for include in library.include_paths {
            build.include(include);
        }
    }
    build.compile("waywallen_snapshot");
}

fn build_pulse_adapter(manifest_dir: &Path) {
    let source = manifest_dir.join("src/system/audio/pulse_adapter.c");
    let header = manifest_dir.join("src/system/audio/pulse_adapter.h");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", header.display());

    let pulse = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("libpulse")
        .expect("libpulse headers are required to build the optional runtime adapter");
    let mut build = cc::Build::new();
    build.file(source).flag_if_supported("-std=gnu11");
    for include in pulse.include_paths {
        build.include(include);
    }
    build.compile("waywallen_pulse_adapter");
    println!("cargo:rustc-link-lib=dl");
}

fn gen_rust(xml_path: &Path, out_file: &Path) {
    println!("cargo:rerun-if-changed={}", xml_path.display());
    let xml =
        fs::read_to_string(xml_path).unwrap_or_else(|e| panic!("read {}: {e}", xml_path.display()));
    let code = wayproto_gen::emit_rust_from_xml(&xml)
        .unwrap_or_else(|e| panic!("wayproto-gen failed on {}: {e}", xml_path.display()));
    fs::write(out_file, code).unwrap_or_else(|e| panic!("write {}: {e}", out_file.display()));
}

fn gen_tray_i18n(i18n_dir: &Path, out_file: &Path) {
    println!("cargo:rerun-if-changed=tools/tray_i18n.rs");
    println!("cargo:rerun-if-changed={}", i18n_dir.display());

    let mut ts_files: Vec<PathBuf> = fs::read_dir(i18n_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", i18n_dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("read entry in {}: {e}", i18n_dir.display()))
                .path()
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("waywallen_") && name.ends_with(".ts"))
                .unwrap_or(false)
        })
        .collect();
    ts_files.sort();

    let mut expected_sources: Option<Vec<String>> = None;
    let mut catalogs: Vec<(String, Vec<(String, String)>)> = Vec::new();

    for path in ts_files {
        println!("cargo:rerun-if-changed={}", path.display());

        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_else(|| panic!("invalid translation filename: {}", path.display()));
        let locale = stem
            .strip_prefix("waywallen_")
            .unwrap_or_else(|| panic!("invalid translation filename: {}", path.display()))
            .replace('-', "_")
            .to_ascii_lowercase();

        let xml =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let context = find_ts_context(&xml, "TrayMenu")
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
            .unwrap_or_else(|| panic!("{} is missing the TrayMenu context", path.display()));
        let mut messages = parse_ts_messages(context)
            .unwrap_or_else(|e| panic!("parse TrayMenu in {}: {e}", path.display()));
        messages.sort_by(|a, b| a.0.cmp(&b.0));

        for pair in messages.windows(2) {
            if pair[0].0 == pair[1].0 {
                panic!(
                    "duplicate TrayMenu source {:?} in {}",
                    pair[0].0,
                    path.display()
                );
            }
        }

        let sources: Vec<String> = messages.iter().map(|(source, _)| source.clone()).collect();
        if let Some(expected) = &expected_sources {
            assert_eq!(
                &sources,
                expected,
                "TrayMenu source messages differ in {}",
                path.display()
            );
        } else {
            expected_sources = Some(sources);
        }

        let entries: Vec<(String, String)> = messages
            .into_iter()
            .filter_map(|(source, translation)| {
                translation.map(|translation| (source, translation))
            })
            .collect();
        catalogs.push((locale, entries));
    }

    if catalogs.is_empty() {
        panic!("no ui/i18n/waywallen_*.ts translation files found");
    }

    let mut code = String::new();
    writeln!(
        code,
        "// @generated by build.rs from the TrayMenu context in ui/i18n/waywallen_*.ts."
    )
    .unwrap();
    writeln!(code, "// Do not edit this file directly.").unwrap();
    writeln!(code, "const GENERATED_CATALOGS: &[(&str, Catalog)] = &[").unwrap();
    for (locale, entries) in catalogs {
        writeln!(code, "    ({locale:?}, &[").unwrap();
        for (source, translation) in entries {
            writeln!(code, "        ({source:?}, {translation:?}),").unwrap();
        }
        writeln!(code, "    ]),").unwrap();
    }
    writeln!(code, "];").unwrap();

    fs::write(out_file, code).unwrap_or_else(|e| panic!("write {}: {e}", out_file.display()));
}
