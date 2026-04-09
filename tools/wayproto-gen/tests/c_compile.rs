//! Integration test: generate the C header + source from the real
//! `waywallen-display-v1.xml` protocol description, hand them to gcc
//! under the same strict flags the rest of the C side uses, and also
//! compile-and-run a small round-trip program that exercises encode →
//! decode → free on a representative subset of the messages.
//!
//! The test is skipped (not failed) if `gcc` is not on PATH — some
//! minimal CI images lack a C toolchain.

use std::path::PathBuf;
use std::process::Command;

fn gcc_available() -> bool {
    Command::new("gcc")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn protocol_xml() -> PathBuf {
    // tools/wayproto-gen/ → ../../protocol/...
    manifest_dir()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("protocol/waywallen_display_v1.xml")
}

#[test]
fn generated_c_compiles_cleanly() {
    if !gcc_available() {
        eprintln!("skipping: gcc not found on PATH");
        return;
    }
    let xml_path = protocol_xml();
    let xml = std::fs::read_to_string(&xml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", xml_path.display()));
    let header = wayproto_gen::emit_c_header_from_xml(&xml).expect("codegen header");
    let source = wayproto_gen::emit_c_source_from_xml(&xml).expect("codegen source");

    let tmp = std::env::temp_dir().join(format!(
        "wayproto-gen-c-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let h_path = tmp.join("ww_proto.h");
    let c_path = tmp.join("ww_proto.c");
    let o_path = tmp.join("ww_proto.o");
    std::fs::write(&h_path, header).unwrap();
    std::fs::write(&c_path, source).unwrap();

    let out = Command::new("gcc")
        .args([
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wpedantic",
            "-Wconversion",
            "-Wsign-conversion",
            "-std=c11",
            "-I",
        ])
        .arg(&tmp)
        .arg("-c")
        .arg(&c_path)
        .arg("-o")
        .arg(&o_path)
        .output()
        .expect("gcc failed to spawn");

    if !out.status.success() {
        panic!(
            "gcc failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Clean up artefacts.
    let _ = std::fs::remove_file(&o_path);
}

#[test]
fn roundtrip_hello_and_bind_buffers() {
    if !gcc_available() {
        eprintln!("skipping: gcc not found on PATH");
        return;
    }
    let xml_path = protocol_xml();
    let xml = std::fs::read_to_string(&xml_path).unwrap();
    let header = wayproto_gen::emit_c_header_from_xml(&xml).expect("codegen header");
    let source = wayproto_gen::emit_c_source_from_xml(&xml).expect("codegen source");

    let tmp = std::env::temp_dir().join(format!(
        "wayproto-gen-c-rt-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let h_path = tmp.join("ww_proto.h");
    let c_path = tmp.join("ww_proto.c");
    let rt_c_path = tmp.join("roundtrip.c");
    let bin_path = tmp.join("roundtrip");
    std::fs::write(&h_path, header).unwrap();
    std::fs::write(&c_path, source).unwrap();

    let rt_src = r#"
#define _POSIX_C_SOURCE 200809L
#include "ww_proto.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void test_hello(void) {
    ww_req_hello_t in;
    in.protocol = strdup("waywallen-display-v1");
    in.client_name = strdup("rt-test");
    in.client_version = strdup("0.0.1");

    ww_buf_t buf;
    ww_buf_init(&buf);
    int rc = ww_req_hello_encode(&in, &buf);
    assert(rc == WW_OK);

    ww_req_hello_t out;
    rc = ww_req_hello_decode(buf.data, buf.len, &out);
    assert(rc == WW_OK);
    assert(strcmp(out.protocol, "waywallen-display-v1") == 0);
    assert(strcmp(out.client_name, "rt-test") == 0);
    assert(strcmp(out.client_version, "0.0.1") == 0);

    ww_req_hello_free(&out);
    ww_req_hello_free(&in);
    ww_buf_free(&buf);
}

static void test_bind_buffers(void) {
    ww_evt_bind_buffers_t in;
    memset(&in, 0, sizeof(in));
    in.buffer_generation = 42;
    in.count = 3;
    in.width = 1920;
    in.height = 1080;
    in.fourcc = 0x34325258; /* XR24 */
    in.modifier = 0x0100000000000001ULL;
    in.planes_per_buffer = 1;

    in.stride.count = 3;
    in.stride.data = calloc(3, sizeof(uint32_t));
    in.stride.data[0] = 7680;
    in.stride.data[1] = 7680;
    in.stride.data[2] = 7680;

    in.plane_offset.count = 3;
    in.plane_offset.data = calloc(3, sizeof(uint32_t));

    in.size.count = 3;
    in.size.data = calloc(3, sizeof(uint64_t));
    for (int i = 0; i < 3; i++) in.size.data[i] = 8294400ULL;

    uint32_t fds = ww_evt_bind_buffers_expected_fds(&in);
    assert(fds == 3);

    ww_buf_t buf;
    ww_buf_init(&buf);
    int rc = ww_evt_bind_buffers_encode(&in, &buf);
    assert(rc == WW_OK);

    ww_evt_bind_buffers_t out;
    rc = ww_evt_bind_buffers_decode(buf.data, buf.len, &out);
    assert(rc == WW_OK);
    assert(out.buffer_generation == 42);
    assert(out.count == 3);
    assert(out.width == 1920);
    assert(out.height == 1080);
    assert(out.fourcc == 0x34325258);
    assert(out.modifier == 0x0100000000000001ULL);
    assert(out.planes_per_buffer == 1);
    assert(out.stride.count == 3 && out.stride.data[0] == 7680);
    assert(out.size.count == 3 && out.size.data[0] == 8294400ULL);

    ww_evt_bind_buffers_free(&out);
    ww_evt_bind_buffers_free(&in);
    ww_buf_free(&buf);
}

static void test_register_display_with_kv(void) {
    ww_req_register_display_t in;
    memset(&in, 0, sizeof(in));
    in.name = strdup("DP-1");
    in.width = 2560;
    in.height = 1440;
    in.refresh_mhz = 144000;
    in.properties.count = 2;
    in.properties.data = calloc(2, sizeof(ww_kv_t));
    in.properties.data[0].key = strdup("scale");
    in.properties.data[0].value = strdup("1.5");
    in.properties.data[1].key = strdup("hdr");
    in.properties.data[1].value = strdup("false");

    ww_buf_t buf;
    ww_buf_init(&buf);
    assert(ww_req_register_display_encode(&in, &buf) == WW_OK);

    ww_req_register_display_t out;
    assert(ww_req_register_display_decode(buf.data, buf.len, &out) == WW_OK);
    assert(strcmp(out.name, "DP-1") == 0);
    assert(out.width == 2560);
    assert(out.refresh_mhz == 144000);
    assert(out.properties.count == 2);
    assert(strcmp(out.properties.data[0].key, "scale") == 0);
    assert(strcmp(out.properties.data[1].value, "false") == 0);

    ww_req_register_display_free(&out);
    ww_req_register_display_free(&in);
    ww_buf_free(&buf);
}

static void test_set_config(void) {
    ww_evt_set_config_t in = {0};
    in.config_generation = 7;
    in.source_rect.x = 0.f;
    in.source_rect.y = 0.f;
    in.source_rect.w = 1920.f;
    in.source_rect.h = 1080.f;
    in.dest_rect.x = 10.f;
    in.dest_rect.y = 20.f;
    in.dest_rect.w = 1900.f;
    in.dest_rect.h = 1060.f;
    in.transform = 0;
    in.clear_r = 0.f;
    in.clear_g = 0.f;
    in.clear_b = 0.f;
    in.clear_a = 1.f;

    ww_buf_t buf;
    ww_buf_init(&buf);
    assert(ww_evt_set_config_encode(&in, &buf) == WW_OK);

    ww_evt_set_config_t out;
    assert(ww_evt_set_config_decode(buf.data, buf.len, &out) == WW_OK);
    assert(out.config_generation == 7);
    assert(out.source_rect.w == 1920.f);
    assert(out.dest_rect.x == 10.f);
    assert(out.clear_a == 1.f);

    ww_evt_set_config_free(&out);
    ww_buf_free(&buf);
}

static void test_bye_empty(void) {
    ww_req_bye_t in = {0};
    ww_buf_t buf;
    ww_buf_init(&buf);
    assert(ww_req_bye_encode(&in, &buf) == WW_OK);
    assert(buf.len == 0);

    ww_req_bye_t out;
    assert(ww_req_bye_decode(buf.data, buf.len, &out) == WW_OK);
    ww_req_bye_free(&out);
    ww_buf_free(&buf);
}

int main(void) {
    test_hello();
    test_bind_buffers();
    test_register_display_with_kv();
    test_set_config();
    test_bye_empty();
    printf("wayproto-gen C roundtrip: OK\n");
    return 0;
}
"#;
    std::fs::write(&rt_c_path, rt_src).unwrap();

    let out = Command::new("gcc")
        .args([
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wpedantic",
            "-std=c11",
            "-I",
        ])
        .arg(&tmp)
        .arg(&c_path)
        .arg(&rt_c_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("gcc spawn");
    if !out.status.success() {
        panic!(
            "gcc failed building roundtrip:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let run = Command::new(&bin_path).output().expect("run roundtrip");
    if !run.status.success() {
        panic!(
            "roundtrip test failed: {}\nstdout: {}\nstderr: {}",
            run.status,
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("wayproto-gen C roundtrip: OK"),
        "unexpected stdout: {stdout}"
    );

    // Clean up.
    let _ = std::fs::remove_file(&bin_path);
}
