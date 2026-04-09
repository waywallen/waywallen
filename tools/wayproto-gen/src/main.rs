//! wayproto-gen — XML protocol description → Rust / C codegen.
//!
//! Phase 1 only supports `--out-rust`. C codegen is deferred to Phase 2.
//!
//! Usage:
//!     wayproto-gen --in <xml> --out-rust <file>
//!
//! The same logic is exposed as a library (`wayproto_gen::`) for
//! in-process use from a `build.rs`.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use wayproto_gen::{codegen_rust, parser};

struct Args {
    input: PathBuf,
    out_rust: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut input = None;
    let mut out_rust = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--in" => input = it.next().map(PathBuf::from),
            "--out-rust" => out_rust = it.next().map(PathBuf::from),
            "-h" | "--help" => {
                eprintln!("usage: wayproto-gen --in <xml> --out-rust <file>");
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    let input = input.ok_or("missing --in")?;
    Ok(Args { input, out_rust })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let src = fs::read_to_string(&args.input)
        .map_err(|e| format!("read {}: {e}", args.input.display()))?;
    let proto = parser::parse_protocol(&src).map_err(|e| e.to_string())?;

    if let Some(path) = &args.out_rust {
        let code = codegen_rust::emit(&proto);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::write(path, code).map_err(|e| format!("write {}: {e}", path.display()))?;
        eprintln!(
            "wayproto-gen: wrote {} ({} requests, {} events)",
            path.display(),
            proto.requests.len(),
            proto.events.len()
        );
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wayproto-gen: error: {e}");
            ExitCode::FAILURE
        }
    }
}
