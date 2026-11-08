use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub path: String,
    pub kind: String,
    pub size: Option<i64>,
    #[serde(default)]
    pub mtime_ns: Option<i64>,
    #[serde(default)]
    pub device: Option<u64>,
    #[serde(default)]
    pub inode: Option<u64>,
}

pub fn snapshot(path: &str) -> Dependency {
    let meta = std::fs::metadata(path).ok();
    Dependency {
        path: path.to_owned(),
        kind: meta
            .as_ref()
            .map_or("missing", |m| if m.is_dir() { "directory" } else { "file" })
            .into(),
        size: meta.as_ref().and_then(|m| i64::try_from(m.len()).ok()),
        mtime_ns: meta.as_ref().and_then(|m| {
            m.mtime()
                .checked_mul(1_000_000_000)?
                .checked_add(m.mtime_nsec())
        }),
        device: meta.as_ref().map(|m| m.dev()),
        inode: meta.as_ref().map(|m| m.ino()),
    }
}

pub fn current(dependencies: &[Dependency]) -> Vec<Dependency> {
    dependencies.iter().map(|d| snapshot(&d.path)).collect()
}

pub fn signature(kind: &str, dependencies: &[Dependency]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted = dependencies.to_vec();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    sorted.dedup_by(|a, b| a.path == b.path);
    let bytes = serde_json::to_vec(&(1, kind, sorted)).expect("serialize probe input");
    format!("{:x}", Sha256::digest(bytes))
}
