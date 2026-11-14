use std::env;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const FILE_NAME: &str = "org.waywallen.waywallen.desktop";
const OWNER_KEY: &str = "X-Waywallen-Autostart";

pub(super) struct DesktopAutostart {
    path: PathBuf,
    system_paths: Vec<PathBuf>,
}

impl DesktopAutostart {
    pub(super) fn new() -> Result<Self> {
        if env::var_os("SNAP").is_some()
            || Path::new("/run/.containerenv").exists()
            || env::var_os("container").is_some()
        {
            return Err(Error::FailedPrecondition(
                "Login startup is unavailable from this sandbox or container".into(),
            ));
        }
        let config = env::var_os("XDG_CONFIG_HOME")
            .filter(|path| Path::new(path).is_absolute())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .filter(|path| path.is_absolute())
            .ok_or_else(|| Error::FailedPrecondition("No user configuration directory".into()))?;
        let dirs = env::var_os("XDG_CONFIG_DIRS")
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/etc/xdg".into());
        Ok(Self {
            path: config.join("autostart").join(FILE_NAME),
            system_paths: env::split_paths(&dirs)
                .filter(|path| path.is_absolute())
                .map(|path| path.join("autostart").join(FILE_NAME))
                .collect(),
        })
    }

    pub(super) fn enabled(&self) -> Result<bool> {
        let entry = match read_entry(&self.path)? {
            Some(entry) => Some(entry),
            None => self.system_entry()?,
        };
        let Some(entry) = entry else { return Ok(false) };
        if value(&entry, "Hidden") == Some("true") {
            return Ok(false);
        }
        let desktop = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let matches = |list: &str| {
            list.split(';')
                .any(|name| !name.is_empty() && desktop.split(':').any(|current| current == name))
        };
        if value(&entry, "OnlyShowIn").is_some_and(|list| !matches(list))
            || value(&entry, "NotShowIn").is_some_and(matches)
        {
            return Ok(false);
        }
        if let Some(executable) = value(&entry, "TryExec").filter(|value| !value.is_empty()) {
            let executable = unescape(executable);
            if !executable_available(&executable) {
                return Ok(false);
            }
        }
        Ok(value(&entry, "Type") == Some("Application")
            && value(&entry, "Exec").is_some_and(|command| !command.is_empty()))
    }

    pub(super) fn ensure_writable(&self) -> Result<()> {
        let legacy = self.path.with_file_name("waywallen.desktop");
        if read_entry(&legacy)?.is_some_and(|entry| {
            value(&entry, "Name") == Some("Waywallen") && value(&entry, "Hidden") != Some("true")
        }) {
            return Err(Error::FailedPrecondition(format!(
                "An existing login startup entry must be disabled first: {}",
                legacy.display()
            )));
        }
        if let Some(entry) = read_entry(&self.path)? {
            if value(&entry, OWNER_KEY) != Some("true") {
                return Err(Error::FailedPrecondition(format!(
                    "Login startup is managed outside Waywallen: {}",
                    self.path.display()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.ensure_writable()?;
        if enabled {
            self.write(&launcher()?)
        } else if self.system_entry()?.is_some() {
            self.write_entry(&format!(
                "[Desktop Entry]\nType=Application\nName=Waywallen\nHidden=true\n{OWNER_KEY}=true\n"
            ))
        } else {
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }

    pub(super) fn refresh(&self) -> Result<()> {
        if let Some(entry) = read_entry(&self.path)? {
            if value(&entry, OWNER_KEY) == Some("true") && value(&entry, "Hidden") != Some("true") {
                let launcher = launcher()?;
                if value(&entry, "Exec") != Some(exec(&launcher)?.as_str()) {
                    self.write(&launcher)?;
                }
            }
        }
        Ok(())
    }

    fn system_entry(&self) -> Result<Option<String>> {
        for path in &self.system_paths {
            if let Some(entry) = read_entry(path)? {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    fn write(&self, launcher: &Path) -> Result<()> {
        let command = exec(launcher)?;
        let executable = launcher.to_str().ok_or_else(|| {
            Error::FailedPrecondition("Startup executable path is not valid UTF-8".into())
        })?;
        let try_exec = executable
            .replace('\\', "\\\\")
            .replace(' ', "\\s")
            .replace('\t', "\\t");
        self.write_entry(&format!(
            "[Desktop Entry]\nType=Application\nName=Waywallen\nExec={command}\nTryExec={try_exec}\nIcon=org.waywallen.waywallen\nTerminal=false\n{OWNER_KEY}=true\n"
        ))
    }

    fn write_entry(&self, content: &str) -> Result<()> {
        let directory = self.path.parent().expect("autostart directory");
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(content.as_bytes())?;
        file.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }
}

pub(super) fn launcher() -> Result<PathBuf> {
    let path = env::var_os("APPIMAGE")
        .or_else(|| env::var_os("WAYWALLEN_LAUNCHER"))
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(env::current_exe)?;
    let temporary = env::temp_dir();
    if !path.is_absolute()
        || path.starts_with(&temporary)
        || path.starts_with("/tmp")
        || path.starts_with("/var/tmp")
        || path
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with(".mount_"))
        || !executable_available(path.as_os_str())
    {
        return Err(Error::FailedPrecondition(
            "Login startup requires a persistent installed executable or AppImage".into(),
        ));
    }
    exec(&path)?;
    Ok(path)
}

fn exec(path: &Path) -> Result<String> {
    let path = path.to_str().ok_or_else(|| {
        Error::FailedPrecondition("Startup executable path is not valid UTF-8".into())
    })?;
    if path.contains(['\n', '\r', '\0', '=']) {
        return Err(Error::FailedPrecondition(
            "Unsupported startup executable path".into(),
        ));
    }
    // env avoids launchers checking the executable before expanding literal %%.
    let mut quoted = String::from("env \"");
    for ch in path.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '$' | '`' => {
                quoted.push_str("\\\\");
                quoted.push(ch);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(ch),
        }
    }
    quoted.push_str("\" --no-ui");
    Ok(quoted)
}

fn executable_available(path: impl AsRef<std::ffi::OsStr>) -> bool {
    let path = Path::new(path.as_ref());
    let executable = |path: &Path| {
        fs::metadata(path).is_ok_and(|metadata| {
            metadata.is_file() && nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
        })
    };
    if path.is_absolute() {
        executable(path)
    } else if path.components().count() == 1 {
        env::var_os("PATH").is_some_and(|paths| {
            env::split_paths(&paths).any(|directory| executable(&directory.join(path)))
        })
    } else {
        false
    }
}

fn read_entry(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(entry) => Ok(Some(entry)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn value<'a>(entry: &'a str, key: &str) -> Option<&'a str> {
    let mut desktop_entry = false;
    entry.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('[') {
            desktop_entry = line == "[Desktop Entry]";
        }
        if !desktop_entry || line.starts_with('#') {
            return None;
        }
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim())
    })
}

fn unescape(value: &str) -> String {
    let mut result = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            result.push(match chars.next() {
                Some('s') => ' ',
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some(ch) => ch,
                None => '\\',
            });
        } else {
            result.push(ch);
        }
    }
    result
}
