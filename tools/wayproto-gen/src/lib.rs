//! wayproto-gen — library entry point.
//!
//! The same code powers both the `wayproto-gen` CLI binary (for
//! shell invocation) and in-process use from a `build.rs` that wants
//! to call the parser and codegen directly without spawning a
//! subprocess.

pub mod codegen_c;
pub mod codegen_rust;
pub mod parser;

/// Convenience: parse an XML source and emit the Rust codegen output
/// in one call.
pub fn emit_rust_from_xml(src: &str) -> Result<String, parser::ParseError> {
    let proto = parser::parse_protocol(src)?;
    Ok(codegen_rust::emit(&proto))
}

/// Convenience: parse an XML source and emit the C header.
pub fn emit_c_header_from_xml(src: &str) -> Result<String, parser::ParseError> {
    let proto = parser::parse_protocol(src)?;
    Ok(codegen_c::emit_header(&proto))
}

/// Convenience: parse an XML source and emit the C source.
pub fn emit_c_source_from_xml(src: &str) -> Result<String, parser::ParseError> {
    let proto = parser::parse_protocol(src)?;
    Ok(codegen_c::emit_source(&proto))
}
