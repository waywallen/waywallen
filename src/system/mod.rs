use std::fs;
use std::path::Path;

pub mod audio;
pub mod autostart;
pub(crate) mod dbus;
pub(crate) mod gamemode;
mod gpu;
pub(crate) mod mpris;
pub(crate) mod notifications;
pub(crate) mod power;
pub(crate) mod session;
pub(crate) mod tray;
pub(crate) mod wallpaper;

pub use gpu::{format_drm_dev, parse_drm_dev, GpuInfo, GPU_DRM_DEV_KEY, RENDER_NODE_KEY};

#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    os_name: String,
    gpus: Vec<GpuInfo>,
}

impl SystemInfo {
    pub fn load() -> Self {
        let os_name = match fs::read_to_string("/etc/os-release") {
            Ok(content) => parse_os_name(&content).unwrap_or_default(),
            Err(error) => {
                log::warn!("read /etc/os-release: {error}");
                String::new()
            }
        };
        Self {
            os_name,
            gpus: gpu::enumerate(),
        }
    }

    pub fn os_name(&self) -> &str {
        &self.os_name
    }

    pub fn gpus(&self) -> &[GpuInfo] {
        &self.gpus
    }

    pub fn has_render_device(&self, drm_dev: &str) -> bool {
        self.render_node_for_drm_dev(drm_dev).is_some()
    }

    pub fn render_node_for_drm_dev(&self, drm_dev: &str) -> Option<&Path> {
        let (major, minor) = parse_drm_dev(drm_dev)?;
        self.gpus
            .iter()
            .find(|gpu| gpu.matches_render(major, minor))?
            .render_node
            .as_deref()
    }

    #[cfg(test)]
    fn load_from(path: &Path) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(Self {
            os_name: parse_os_name(&content).unwrap_or_default(),
            gpus: Vec::new(),
        })
    }
}

fn parse_os_name(content: &str) -> Option<String> {
    content.lines().filter_map(parse_name_line).last()
}

fn parse_name_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    if key.trim() != "NAME" {
        return None;
    }
    parse_value(value.trim())
}

fn parse_value(value: &str) -> Option<String> {
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return unescape(inner);
    }
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return Some(inner.to_owned());
    }
    unescape(value)
}

fn unescape(value: &str) -> Option<String> {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            result.push(chars.next()?);
        } else {
            result.push(ch);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn parses_os_release_name() {
        assert_eq!(
            parse_os_name("ID=fedora\nNAME=\"Fedora Linux\"\n"),
            Some("Fedora Linux".to_owned())
        );
        assert_eq!(parse_os_name("NAME=Ubuntu\n"), Some("Ubuntu".to_owned()));
        assert_eq!(
            parse_os_name("NAME='Arch Linux'\n"),
            Some("Arch Linux".to_owned())
        );
        assert_eq!(
            parse_os_name("NAME=First\nNAME=Second\\ Linux\n"),
            Some("Second Linux".to_owned())
        );
    }

    #[test]
    fn loads_os_release_from_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "NAME=\"Test Linux\"").unwrap();

        let info = SystemInfo::load_from(file.path()).unwrap();

        assert_eq!(info.os_name, "Test Linux");
    }

    #[test]
    fn resolves_render_node_from_system_snapshot() {
        let info = SystemInfo {
            os_name: String::new(),
            gpus: vec![GpuInfo {
                render_node: Some(PathBuf::from("/dev/dri/renderD128")),
                render_major: 226,
                render_minor: 128,
                ..Default::default()
            }],
        };

        assert_eq!(
            info.render_node_for_drm_dev("226:128"),
            Some(Path::new("/dev/dri/renderD128"))
        );
        assert!(!info.has_render_device("226:129"));
        assert!(!info.has_render_device("invalid"));
    }
}
