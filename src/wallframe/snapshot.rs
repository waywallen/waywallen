use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use tokio::sync::watch;

use super::renderer_manager::{DrmNode, PublishedPool};

unsafe extern "C" {
    fn ww_snapshot_read(
        major: u32,
        minor: u32,
        width: u32,
        height: u32,
        fourcc: u32,
        modifier: u64,
        planes: u32,
        fds: *const RawFd,
        offsets: *const u32,
        strides: *const u32,
        fence: RawFd,
        rgba: *mut u8,
        error: *mut std::ffi::c_char,
        error_len: usize,
    ) -> i32;
}

pub(crate) struct Snapshot {
    state: Mutex<CaptureState>,
    result: watch::Sender<Option<std::result::Result<Arc<std::fs::File>, String>>>,
}

#[derive(Default)]
struct CaptureState {
    requested: bool,
    running: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: Mutex::new(CaptureState::default()),
            result: watch::channel(None).0,
        }
    }
}

impl Snapshot {
    pub(crate) fn requested(&self) -> bool {
        self.state.lock().unwrap().requested
    }

    pub(crate) async fn request(&self) {
        let mut result = self.result.subscribe();
        loop {
            {
                let mut state = self.state.lock().unwrap();
                if !state.running {
                    state.requested = true;
                    self.result.send_replace(None);
                    return;
                }
            }
            // An abandoned readback still owns its buffer until the worker exits.
            let _ = result.changed().await;
        }
    }

    pub(crate) fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.requested = false;
        self.result.send_replace(None);
    }

    pub(crate) fn needs_capture(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.requested && !state.running && self.result.borrow().is_none()
    }

    pub(crate) fn spawn(
        self: &Arc<Self>,
        member: super::sync::FrameConsumerMember,
        work: impl FnOnce(super::sync::FrameConsumerMember) -> Result<std::fs::File> + Send + 'static,
    ) {
        let mut state = self.state.lock().unwrap();
        if !state.requested || state.running || self.result.borrow().is_some() {
            return;
        }
        state.running = true;
        let snapshot = Arc::clone(self);
        if let Err(error) = std::thread::Builder::new()
            .name("wallpaper-snapshot".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(member)))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("snapshot worker panicked")))
                        .map(Arc::new)
                        .map_err(|error| format!("{error:#}"));
                let mut state = snapshot.state.lock().unwrap();
                state.running = false;
                snapshot
                    .result
                    .send_replace(state.requested.then_some(result));
            })
        {
            state.running = false;
            self.result.send_replace(Some(Err(error.to_string())));
        }
    }

    pub(crate) async fn wait(&self) -> Result<Arc<std::fs::File>> {
        let mut result = self.result.subscribe();
        loop {
            if let Some(result) = result.borrow_and_update().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            result.changed().await.context("renderer snapshot closed")?;
        }
    }
}

pub(crate) fn source_image(wp_type: &str, resource: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(resource);
    // Wallpaper definitions need their resolved frame, not the XML/JSON source file.
    let definition = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("xml") || value.eq_ignore_ascii_case("json")
        });
    (wp_type.eq_ignore_ascii_case("image") && !definition && path.is_file()).then(|| path.into())
}

pub(crate) fn capture(
    gpu: DrmNode,
    pool: &PublishedPool,
    index: u32,
    fence: RawFd,
    member: super::sync::FrameConsumerMember,
) -> Result<std::fs::File> {
    let bytes = rgba_size(pool.width, pool.height)?;
    let planes = pool.planes_per_buffer as usize;
    if index >= pool.count || !(1..=4).contains(&planes) {
        bail!("invalid snapshot buffer index or plane count");
    }
    let start = (index as usize)
        .checked_mul(planes)
        .context("snapshot plane index overflow")?;
    let end = start
        .checked_add(planes)
        .context("snapshot plane count overflow")?;
    if end > pool.fds.len() || end > pool.plane_offset.len() || end > pool.stride.len() {
        bail!("incomplete snapshot buffer planes");
    }
    let fds = pool.fds[start..end]
        .iter()
        .map(AsRawFd::as_raw_fd)
        .collect::<Vec<_>>();
    let mut rgba = vec![0u8; bytes];
    let mut error = [0i8; 256];
    // SAFETY: arrays contain one entry per validated plane; output has width*height*4 bytes.
    let status = unsafe {
        ww_snapshot_read(
            gpu.major,
            gpu.minor,
            pool.width,
            pool.height,
            pool.fourcc,
            pool.modifier,
            pool.planes_per_buffer,
            fds.as_ptr(),
            pool.plane_offset[start..end].as_ptr(),
            pool.stride[start..end].as_ptr(),
            fence,
            rgba.as_mut_ptr(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if status != 0 {
        // SAFETY: the adapter always terminates the error buffer.
        bail!(
            "{}",
            unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
        );
    }
    // GPU access is finished; disk I/O must not hold up the producer's release timeline.
    drop(member);
    // Anonymous files cannot leave stale snapshots behind after a process exit.
    let mut file = tempfile::tempfile()?;
    write_bmp(&mut file, pool.width, pool.height, &rgba)?;
    Ok(file)
}

fn rgba_size(width: u32, height: u32) -> Result<usize> {
    // Bound allocations from renderer-controlled metadata to 64 megapixels.
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || pixels > 64 * 1024 * 1024 {
        bail!("invalid snapshot dimensions {width}x{height}");
    }
    Ok(pixels as usize * 4)
}

fn write_bmp(out: &mut impl Write, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if rgba.len() != rgba_size(width, height)? {
        bail!("snapshot pixel length does not match dimensions");
    }
    let stride = (width * 3 + 3) & !3;
    let mut header = [0u8; 54];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(54 + stride * height).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&width.to_le_bytes());
    // DMA-BUF row zero is the top row, even when read through OpenGL.
    header[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&24u16.to_le_bytes());
    out.write_all(&header)?;
    let mut row = vec![0u8; stride as usize];
    for source in rgba.chunks_exact(width as usize * 4) {
        for (pixel, target) in source.chunks_exact(4).zip(row.chunks_exact_mut(3)) {
            target.copy_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        out.write_all(&row)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member() -> (
        super::super::sync::FrameConsumerMember,
        tokio::sync::mpsc::UnboundedReceiver<super::super::sync::FrameRecord>,
    ) {
        use super::super::sync::{register_frame, FrameConsumerIdentity, FrameIdentity};
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let frame = FrameIdentity {
            buffer_generation: 1,
            buffer_index: 0,
            release_point: 1,
        };
        let member = register_frame(
            &sender,
            frame,
            vec![FrameConsumerIdentity {
                frame,
                renderer_id: "snapshot-test".into(),
                display_id: 0,
                display_session_id: 0,
                display_name: "snapshot".into(),
                frame_seq: 1,
            }],
        )
        .unwrap()
        .pop()
        .unwrap();
        (member, receiver)
    }

    #[test]
    fn ordinary_images_use_the_source_without_capture() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("image.png");
        std::fs::write(&file, b"image").unwrap();
        assert_eq!(
            source_image("image", file.to_str().unwrap()),
            Some(file.clone())
        );
        assert!(source_image("video", file.to_str().unwrap()).is_none());
        assert!(source_image("image", directory.path().to_str().unwrap()).is_none());
        for name in ["background.xml", "metadata.json", "background.XML"] {
            let definition = directory.path().join(name);
            std::fs::write(&definition, b"wallpaper definition").unwrap();
            assert!(source_image("image", definition.to_str().unwrap()).is_none());
        }
        let snapshot = Arc::new(Snapshot::default());
        assert!(!snapshot.needs_capture());
        snapshot.spawn(member().0, |_| {
            panic!("ordinary images must not be read back")
        });
        assert!(!snapshot.state.lock().unwrap().running);
    }

    #[tokio::test]
    async fn readback_runs_in_background_and_keeps_its_buffer_lease() {
        let snapshot = Arc::new(Snapshot::default());
        snapshot.request().await;
        let (member, mut records) = member();
        assert!(matches!(
            records.try_recv().unwrap(),
            super::super::sync::FrameRecord::Register { .. }
        ));
        let (started, running) = std::sync::mpsc::channel();
        let (finish, wait) = std::sync::mpsc::channel();
        snapshot.spawn(member, move |_member| {
            started.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let mut file = tempfile::tempfile()?;
            file.write_all(b"frame")?;
            Ok(file)
        });
        running
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(snapshot.result.borrow().is_none());
        assert!(
            records.try_recv().is_err(),
            "producer buffer was released during readback"
        );
        finish.send(()).unwrap();
        assert!(snapshot.wait().await.is_ok());
        assert!(matches!(
            records.recv().await.unwrap(),
            super::super::sync::FrameRecord::Skipped { .. }
        ));
    }

    #[test]
    fn bmp_preserves_row_order_colors_and_padding() {
        let mut data = Vec::new();
        write_bmp(&mut data, 1, 2, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(data.len(), 62);
        assert_eq!(&data[22..26], &(-2i32).to_le_bytes());
        assert_eq!(&data[54..], &[0, 0, 255, 0, 255, 0, 0, 0]);
        assert!(write_bmp(&mut Vec::new(), 1, 2, &[0; 4]).is_err());
        assert!(rgba_size(u32::MAX, u32::MAX).is_err());
    }

    #[tokio::test]
    async fn cancelled_capture_keeps_its_lease_until_the_worker_releases_it() {
        let snapshot = Arc::new(Snapshot::default());
        snapshot.request().await;
        let (started, running) = std::sync::mpsc::channel();
        let (finish, wait) = std::sync::mpsc::channel();
        snapshot.spawn(member().0, move |_member| {
            started.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(tempfile::tempfile()?)
        });
        running
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        snapshot.cancel();
        assert!(!snapshot.needs_capture());
        let next = snapshot.request();
        tokio::pin!(next);
        assert!(futures_util::poll!(&mut next).is_pending());
        finish.send(()).unwrap();
        next.await;
        assert!(snapshot.result.borrow().is_none());
        assert!(snapshot.needs_capture());
        snapshot.spawn(member().0, |_member| {
            let mut file = tempfile::tempfile()?;
            file.write_all(b"new frame")?;
            Ok(file)
        });
        assert_eq!(snapshot.wait().await.unwrap().metadata().unwrap().len(), 9);
        assert!(!snapshot.needs_capture());
    }

    #[tokio::test]
    async fn invalid_pool_is_rejected_and_snapshot_waiters_are_not_lost() {
        let snapshot = Arc::new(Snapshot::default());
        let pool = PublishedPool {
            generation: 1,
            flags: 0,
            count: 1,
            fourcc: 0,
            width: 1,
            height: 1,
            modifier: 0,
            planes_per_buffer: 1,
            stride: vec![],
            plane_offset: vec![],
            size: vec![],
            fds: vec![],
        };
        assert!(!snapshot.needs_capture());
        snapshot.request().await;
        snapshot.spawn(member().0, move |member| {
            capture(DrmNode::UNKNOWN, &pool, 0, -1, member)
        });
        for _ in 0..2 {
            let error = snapshot.wait().await.unwrap_err().to_string();
            assert!(error.contains("incomplete snapshot buffer planes"));
        }
    }
}
