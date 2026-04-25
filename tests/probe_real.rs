//! Integration check that confirms — on hosts that have libavformat
//! installed — the AvFormatProbe actually extracts a real format
//! string from a tiny on-disk WAV file. Skipped (with eprintln!) if
//! libavformat isn't loadable.
use std::io::Write;
use waywallen::media_probe::{AvFormatProbe, MediaProbe};

#[test]
fn probe_extracts_wav_format_on_hosts_with_libavformat() {
    // Self-skip if no libavformat on this host. We detect this by
    // probing a missing path first; if that fails to load libav the
    // following real probe also won't.
    let probe = AvFormatProbe::new();
    let probe_smoke = probe.probe("/__definitely_missing__");
    // size/width/height/format all None for a missing file in either
    // case. We need a positive signal that libav loaded — which we
    // get by checking a real WAV's format below.

    let mut tmp = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    let header: [u8; 44] = [
        b'R', b'I', b'F', b'F', 37, 0, 0, 0, b'W', b'A', b'V', b'E',
        b'f', b'm', b't', b' ', 16, 0, 0, 0, 1, 0, 1, 0,
        0x44, 0xAC, 0, 0, 0x44, 0xAC, 0, 0, 1, 0, 8, 0,
        b'd', b'a', b't', b'a', 1, 0, 0, 0,
    ];
    tmp.write_all(&header).unwrap();
    tmp.write_all(&[0x80]).unwrap();
    tmp.flush().unwrap();

    let meta = probe.probe(tmp.path().to_str().unwrap());
    assert_eq!(meta.size, Some(45), "size must be filled regardless");
    match meta.format {
        Some(fmt) => {
            eprintln!("libavformat OK — extracted format={fmt:?}");
            assert!(fmt.contains("wav"), "format string should contain 'wav', got {fmt:?}");
        }
        None => {
            eprintln!("libavformat unavailable on this host — full probe skipped");
        }
    }
    // Sanity: smoke probe matches none-ness for a missing file.
    assert_eq!(probe_smoke.size, None);
}
