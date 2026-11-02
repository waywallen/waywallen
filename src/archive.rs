use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub max_entries: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_depth: usize,
}

pub(crate) struct Extraction {
    pub bytes: u64,
    pub files: usize,
    pub entries: Vec<ExtractedFile>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExtractedFile {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
}

struct Entry {
    path: PathBuf,
    kind: Kind,
    bytes: u64,
}

struct Output {
    kind: Kind,
    explicit: bool,
}

fn relative_path(name: &str, max_depth: usize) -> Result<PathBuf> {
    ensure!(
        !name.contains(['\0', '\\']) && !name.split('/').any(|part| part == ".."),
        "unsafe ZIP entry path {name:?}"
    );
    let mut result = PathBuf::new();
    let mut depth = 0;
    for component in Path::new(name).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                if depth == 0 {
                    let bytes = part.as_encoded_bytes();
                    ensure!(
                        !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'),
                        "Windows drive prefix in ZIP entry {name:?}"
                    );
                }
                depth += 1;
                ensure!(depth <= max_depth, "ZIP entry exceeds path depth limit");
                result.push(part);
            }
            _ => bail!("unsafe ZIP entry path {name:?}"),
        }
    }
    ensure!(depth != 0, "empty ZIP entry path");
    Ok(result)
}

fn register_output(outputs: &mut BTreeMap<PathBuf, Output>, entry: &Entry) -> Result<()> {
    if let Some(previous) = outputs.get_mut(&entry.path) {
        ensure!(
            previous.kind == Kind::Directory && entry.kind == Kind::Directory && !previous.explicit,
            "duplicate or conflicting ZIP output {}",
            entry.path.display()
        );
        previous.explicit = true;
    } else {
        outputs.insert(
            entry.path.clone(),
            Output {
                kind: entry.kind,
                explicit: true,
            },
        );
    }
    for parent in entry.path.ancestors().skip(1) {
        if parent.as_os_str().is_empty() {
            break;
        }
        if let Some(previous) = outputs.get(parent) {
            ensure!(
                previous.kind == Kind::Directory,
                "file/directory conflict in ZIP output {}",
                parent.display()
            );
        } else {
            outputs.insert(
                parent.to_path_buf(),
                Output {
                    kind: Kind::Directory,
                    explicit: false,
                },
            );
        }
    }
    Ok(())
}

fn preflight<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    limits: Limits,
) -> Result<Vec<Entry>> {
    ensure!(
        archive.len() <= limits.max_entries,
        "ZIP exceeds entry count limit"
    );
    let mut entries = Vec::with_capacity(archive.len());
    let mut outputs = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .context("read ZIP entry metadata")?;
        ensure!(!file.encrypted(), "encrypted ZIP entries are unsupported");
        let kind = if file.is_dir() {
            Kind::Directory
        } else {
            Kind::File
        };
        if let Some(mode) = file.unix_mode() {
            let file_type = mode & 0o170000;
            ensure!(
                file_type == 0
                    || (file_type == 0o100000 && kind == Kind::File)
                    || (file_type == 0o040000 && kind == Kind::Directory),
                "ZIP entry is a symlink, special file, or has an inconsistent type"
            );
        }
        let entry = Entry {
            path: relative_path(file.name(), limits.max_depth)?,
            kind,
            bytes: file.size(),
        };
        if kind == Kind::Directory {
            ensure!(entry.bytes == 0, "ZIP directory contains file data");
        } else {
            ensure!(
                entry.bytes <= limits.max_file_bytes,
                "ZIP entry exceeds file size limit"
            );
            total = total
                .checked_add(entry.bytes)
                .context("ZIP size overflow")?;
            ensure!(
                total <= limits.max_total_bytes,
                "ZIP exceeds total size limit"
            );
        }
        register_output(&mut outputs, &entry)?;
        entries.push(entry);
    }
    Ok(entries)
}

fn copy_entry<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_limit: u64,
    total: &mut u64,
    total_limit: u64,
) -> Result<u64> {
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let remaining = (file_limit - bytes).min(total_limit - *total);
        let capacity = remaining.saturating_add(1).min(buffer.len() as u64) as usize;
        let count = reader
            .read(&mut buffer[..capacity])
            .context("read ZIP entry data")?;
        if count == 0 {
            return Ok(bytes);
        }
        ensure!(
            count as u64 <= remaining,
            "ZIP entry exceeds streaming size limit"
        );
        writer
            .write_all(&buffer[..count])
            .context("write ZIP entry data")?;
        bytes += count as u64;
        *total += count as u64;
    }
}

// The caller owns a private destination and removes it on failure. zip exposes
// filename-keyed entries; raw duplicate central-directory records are not visible.
pub(crate) fn extract_zip(
    archive: &Path,
    destination: &Path,
    limits: Limits,
) -> Result<Extraction> {
    let metadata = fs::symlink_metadata(destination).context("inspect ZIP destination")?;
    ensure!(
        metadata.is_dir() && !metadata.is_symlink(),
        "ZIP destination must be a directory"
    );
    ensure!(
        fs::read_dir(destination)
            .context("read ZIP destination")?
            .next()
            .transpose()?
            .is_none(),
        "ZIP destination must be empty"
    );
    let destination = fs::canonicalize(destination).context("resolve ZIP destination")?;
    let file = File::open(archive).context("open ZIP archive")?;
    let mut archive = zip::ZipArchive::new(file).context("read ZIP archive")?;
    let entries = preflight(&mut archive, limits)?;
    let mut result = Extraction {
        bytes: 0,
        files: 0,
        entries: Vec::new(),
    };
    for (index, entry) in entries.iter().enumerate() {
        let path = destination.join(&entry.path);
        let mut file = archive.by_index(index).context("open ZIP entry")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("create ZIP parent directories")?;
        }
        let bytes = match entry.kind {
            Kind::Directory => {
                fs::create_dir_all(&path).context("create ZIP directory")?;
                copy_entry(
                    &mut file,
                    &mut std::io::sink(),
                    0,
                    &mut result.bytes,
                    limits.max_total_bytes,
                )?
            }
            Kind::File => {
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .context("create ZIP output file")?;
                let bytes = copy_entry(
                    &mut file,
                    &mut output,
                    limits.max_file_bytes,
                    &mut result.bytes,
                    limits.max_total_bytes,
                )?;
                result.files += 1;
                result.entries.push(ExtractedFile {
                    path: entry.path.clone(),
                    bytes,
                });
                bytes
            }
        };
        ensure!(
            bytes == entry.bytes,
            "ZIP entry has a truncated or inconsistent uncompressed size"
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    fn limits() -> Limits {
        Limits {
            max_entries: 100,
            max_file_bytes: 1024,
            max_total_bytes: 4096,
            max_depth: 8,
        }
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o777);
        for (name, bytes) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    fn extract(bytes: &[u8], limits: Limits) -> Result<(tempfile::TempDir, Extraction)> {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("model.zip");
        fs::write(&path, bytes).unwrap();
        let output = root.path().join("output");
        fs::create_dir(&output).unwrap();
        let result = extract_zip(&path, &output, limits)?;
        Ok((root, result))
    }

    fn change_central_u32(bytes: &mut [u8], field: usize, value: u32) {
        let archive = zip::ZipArchive::new(Cursor::new(&*bytes)).unwrap();
        let offset = archive.central_directory_start() as usize + field;
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn archive_extracts_regular_files_and_directories_without_executable_permissions() {
        let bytes = archive(&[
            ("scene/scene.gltf", b"{}"),
            ("scene/", b""),
            ("scene/tex.png", b"image"),
        ]);
        let (root, result) = extract(&bytes, limits()).unwrap();
        assert_eq!(result.bytes, 7);
        assert_eq!(result.files, 2);
        let path = root.path().join("output/scene/scene.gltf");
        assert_eq!(fs::read(&path).unwrap(), b"{}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o111, 0);
        }
    }

    #[test]
    fn archive_rejects_unsafe_paths() {
        for name in [
            "../escape",
            "a/../escape",
            "/escape",
            "a\\escape",
            "C:/escape",
            "./C:/escape",
            "a\0b",
            ".",
        ] {
            let bytes = archive(&[(name, b"data")]);
            assert!(extract(&bytes, limits()).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn archive_rejects_symlinks_and_special_files() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("link", "target", SimpleFileOptions::default())
            .unwrap();
        assert!(extract(&writer.finish().unwrap().into_inner(), limits()).is_err());
        for mode in [0o010666u32, 0o020666, 0o060666, 0o140666, 0o040755] {
            let mut bytes = archive(&[("file", b"data")]);
            change_central_u32(&mut bytes, 38, mode << 16);
            assert!(extract(&bytes, limits()).is_err(), "accepted mode {mode:o}");
        }
    }

    #[test]
    fn archive_rejects_normalized_duplicates_and_file_directory_conflicts() {
        for entries in [
            vec![("file", b"a".as_slice()), ("./file", b"b".as_slice())],
            vec![("a//file", b"a".as_slice()), ("a/file", b"b".as_slice())],
            vec![("a", b"a".as_slice()), ("a/file", b"b".as_slice())],
            vec![("a/file", b"a".as_slice()), ("a", b"b".as_slice())],
            vec![("a/", b"".as_slice()), ("a", b"b".as_slice())],
        ] {
            assert!(extract(&archive(&entries), limits()).is_err());
        }
    }

    #[test]
    fn archive_preflights_entry_size_total_and_depth_limits() {
        let bytes = archive(&[("a/file", b"1234"), ("b", b"5678")]);
        for restricted in [
            Limits {
                max_entries: 1,
                ..limits()
            },
            Limits {
                max_file_bytes: 3,
                ..limits()
            },
            Limits {
                max_total_bytes: 7,
                ..limits()
            },
            Limits {
                max_depth: 1,
                ..limits()
            },
        ] {
            let root = tempfile::tempdir().unwrap();
            let input = root.path().join("input.zip");
            let output = root.path().join("output");
            fs::write(&input, &bytes).unwrap();
            fs::create_dir(&output).unwrap();
            assert!(extract_zip(&input, &output, restricted).is_err());
            assert!(fs::read_dir(&output).unwrap().next().is_none());
        }
    }

    #[test]
    fn archive_enforces_streaming_limits_without_trusting_declared_size() {
        let mut bytes = archive(&[("file", &[b'x'; 32])]);
        change_central_u32(&mut bytes, 24, 1);
        let error = extract(
            &bytes,
            Limits {
                max_file_bytes: 16,
                ..limits()
            },
        )
        .err()
        .unwrap();
        assert!(
            error.to_string().contains("streaming size limit"),
            "{error:#}"
        );
        let error = extract(
            &bytes,
            Limits {
                max_total_bytes: 16,
                ..limits()
            },
        )
        .err()
        .unwrap();
        assert!(
            error.to_string().contains("streaming size limit"),
            "{error:#}"
        );
    }

    #[test]
    fn archive_rejects_crc_errors_and_inconsistent_sizes() {
        let mut bytes = archive(&[("file", b"payload")]);
        change_central_u32(&mut bytes, 16, 0);
        assert!(extract(&bytes, limits()).is_err());
        let mut bytes = archive(&[("file", b"payload")]);
        change_central_u32(&mut bytes, 24, 20);
        let error = extract(&bytes, limits()).err().unwrap();
        assert!(
            error.to_string().contains("inconsistent uncompressed size"),
            "{error:#}"
        );
        let mut bytes = archive(&[("file", b"payload")]);
        bytes.truncate(bytes.len() - 10);
        assert!(extract(&bytes, limits()).is_err());
    }

    #[test]
    fn archive_requires_an_existing_empty_real_directory() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input.zip");
        let output = root.path().join("output");
        fs::write(&input, archive(&[("file", b"data")])).unwrap();
        assert!(extract_zip(&input, &output, limits()).is_err());
        fs::create_dir(&output).unwrap();
        fs::write(output.join("existing"), b"keep").unwrap();
        assert!(extract_zip(&input, &output, limits()).is_err());
        assert_eq!(fs::read(output.join("existing")).unwrap(), b"keep");
        #[cfg(unix)]
        {
            let alias = root.path().join("alias");
            std::os::unix::fs::symlink(&output, &alias).unwrap();
            assert!(extract_zip(&input, &alias, limits()).is_err());
        }
    }

    #[test]
    fn archive_accepts_empty_archives_with_zero_budgets() {
        let (_, result) = extract(
            &archive(&[]),
            Limits {
                max_entries: 0,
                max_file_bytes: 0,
                max_total_bytes: 0,
                max_depth: 0,
            },
        )
        .unwrap();
        assert_eq!(result.bytes, 0);
        assert_eq!(result.files, 0);
    }
}
