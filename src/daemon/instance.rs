use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// `flock` on a file next to the display socket. The D-Bus name only
/// guards one session bus; the runtime dir (and the socket in it) can be
/// shared by daemons on different buses. The kernel drops the lock when
/// the process dies, so a crash never leaves a stale lock behind.
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut pid = String::new();
                let _ = file.read_to_string(&mut pid);
                match pid.trim() {
                    "" => anyhow::bail!("waywallen daemon is already running"),
                    pid => anyhow::bail!("waywallen daemon is already running (pid {pid})"),
                }
            }
            Err(TryLockError::Error(e)) => {
                anyhow::bail!("lock {}: {e}", path.display())
            }
        }
        // Informational only: the lock, not the pid, decides ownership.
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = write!(file, "{}", std::process::id());
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_reports_owner_pid() {
        let dir = std::env::temp_dir().join(format!("waywallen-lock-{}", std::process::id()));
        let path = dir.join("daemon.lock");
        let first = InstanceLock::acquire(&path).unwrap();

        let error = InstanceLock::acquire(&path).err().unwrap().to_string();
        assert_eq!(
            error,
            format!(
                "waywallen daemon is already running (pid {})",
                std::process::id()
            )
        );

        drop(first);
        assert!(InstanceLock::acquire(&path).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
