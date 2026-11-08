use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;

const MAX_LINE_BYTES: usize = 8 * 1024;
const MAX_TAIL_BYTES: usize = 32 * 1024;
const MAX_TAIL_LINES: usize = 64;

#[derive(Clone)]
pub(crate) struct ChildStderrCapture {
    tail: Arc<Mutex<StderrTail>>,
    context: Arc<Mutex<String>>,
    done: watch::Receiver<bool>,
}

impl ChildStderrCapture {
    pub(crate) fn spawn<R>(stderr: R, target: String) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let tail = Arc::new(Mutex::new(StderrTail::default()));
        let context = Arc::new(Mutex::new(String::new()));
        let (done_tx, done) = watch::channel(false);
        tokio::spawn(read_stderr(
            stderr,
            target,
            Arc::clone(&tail),
            Arc::clone(&context),
            done_tx,
        ));
        Self {
            tail,
            context,
            done,
        }
    }

    pub(crate) fn set_context(&self, value: String) {
        *self
            .context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
    }

    pub(crate) async fn drain(&self, timeout: Duration) -> StderrSnapshot {
        let mut done = self.done.clone();
        if !*done.borrow() {
            let _ = tokio::time::timeout(timeout, done.changed()).await;
        }
        self.snapshot()
    }

    pub(crate) fn snapshot(&self) -> StderrSnapshot {
        let tail = self
            .tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        StderrSnapshot {
            lines: tail.lines.iter().cloned().collect(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct StderrSnapshot {
    lines: Vec<String>,
}

impl StderrSnapshot {
    pub(crate) fn last_line(&self) -> Option<&str> {
        self.lines.last().map(String::as_str)
    }

    pub(crate) fn last_line_limited(&self, max_bytes: usize) -> Option<String> {
        let line = self.last_line()?;
        let mut end = line.len().min(max_bytes);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        let mut limited = line[..end].to_owned();
        if end != line.len() {
            limited.push('…');
        }
        Some(limited)
    }
}

#[derive(Default)]
struct StderrTail {
    lines: VecDeque<String>,
    bytes: usize,
}

impl StderrTail {
    fn push(&mut self, line: String) {
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_TAIL_LINES || self.bytes > MAX_TAIL_BYTES {
            let Some(removed) = self.lines.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.len());
        }
    }
}

async fn read_stderr<R>(
    mut stderr: R,
    target: String,
    tail: Arc<Mutex<StderrTail>>,
    context: Arc<Mutex<String>>,
    done: watch::Sender<bool>,
) where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 4096];
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) => {
                emit_line(&target, &mut line, truncated, &tail, &context);
                break;
            }
            Ok(read) => {
                for byte in &chunk[..read] {
                    if *byte == b'\n' {
                        emit_line(&target, &mut line, truncated, &tail, &context);
                        truncated = false;
                    } else if line.len() < MAX_LINE_BYTES {
                        line.push(*byte);
                    } else {
                        truncated = true;
                    }
                }
            }
            Err(error) => {
                log::warn!(target: &target, "failed to read stderr: {error}");
                break;
            }
        }
    }
    let _ = done.send(true);
}

fn emit_line(
    target: &str,
    line: &mut Vec<u8>,
    truncated: bool,
    tail: &Arc<Mutex<StderrTail>>,
    context: &Mutex<String>,
) {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let mut message = String::from_utf8_lossy(line).into_owned();
    line.clear();
    if truncated {
        message.push_str(" [truncated]");
    }
    if message.trim().is_empty() {
        return;
    }
    let context = context
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if context.is_empty() {
        log::info!(target: target, "{message}");
    } else {
        log::info!(target: target, "{context}: {message}");
    }
    tail.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn captures_non_utf8_and_bounds_tail() {
        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let capture = ChildStderrCapture::spawn(reader, "renderer test".to_string());
        writer.write_all(b"first\nnon-utf8: \xff\n").await.unwrap();
        for index in 0..80 {
            writer
                .write_all(format!("line-{index}\n").as_bytes())
                .await
                .unwrap();
        }
        drop(writer);

        let snapshot = capture.drain(Duration::from_secs(1)).await;
        assert_eq!(snapshot.lines.len(), MAX_TAIL_LINES);
        assert_eq!(snapshot.last_line(), Some("line-79"));
        assert!(snapshot.lines.iter().all(|line| line != "first"));
    }

    #[tokio::test]
    async fn truncates_unbounded_line() {
        let (mut writer, reader) = tokio::io::duplex(32 * 1024);
        let capture = ChildStderrCapture::spawn(reader, "renderer test".to_string());
        writer
            .write_all(&vec![b'x'; MAX_LINE_BYTES * 2])
            .await
            .unwrap();
        drop(writer);

        let snapshot = capture.drain(Duration::from_secs(1)).await;
        let line = snapshot.last_line().unwrap();
        assert!(line.ends_with(" [truncated]"));
        assert!(line.len() <= MAX_LINE_BYTES + " [truncated]".len());
    }

    #[tokio::test]
    async fn drains_loader_failure_before_process_timeout() {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("printf 'error while loading shared libraries: libmissing.so\\n' >&2; exit 127")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let capture =
            ChildStderrCapture::spawn(child.stderr.take().unwrap(), "renderer test".to_string());

        let status = child.wait().await.unwrap();
        let snapshot = capture.drain(Duration::from_secs(1)).await;
        assert_eq!(status.code(), Some(127));
        assert_eq!(
            snapshot.last_line(),
            Some("error while loading shared libraries: libmissing.so")
        );
    }

    #[test]
    fn limits_failure_line_at_utf8_boundary() {
        let snapshot = StderrSnapshot {
            lines: vec!["一二三".to_string()],
        };
        assert_eq!(snapshot.last_line_limited(4).as_deref(), Some("一…"));
    }
}
