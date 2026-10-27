#[cfg(target_arch = "wasm32")]
use super::services::browser_can_read;
use super::services::{ReadFuture, unreadable};
use super::*;

/// The file types File ▸ Open and a drop on the home screen open.
pub(crate) const OPEN_EXTENSIONS: &[&str] = &["figcraft", "fig", "sketch", "json", "svg", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

/// Whether `path` names a file type that opens as a document (the extension, in any case).
pub(crate) fn opens(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|e| OPEN_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Whether `path` is a whole document (`.figcraft`, `.fig`, `.sketch`, file JSON) rather than an image or SVG:
/// dropped on the canvas it opens as a file of its own instead of adding its pages to the open one.
pub(crate) fn is_document(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|e| ["figcraft", "fig", "sketch", "json"].iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Admission failures share the drop error path; no future, handle or thread is retained.
pub(crate) fn import_busy(name: &str) -> String {
    i18n::trf("Cannot import {name}: too many pending files or import memory limit exceeded.", &[("name", &name)])
}

/// Bound per-gesture processing and error toasts as well as the admitted pending queue.
pub(crate) fn limit_drop(app: &mut FigApp, files: &mut Vec<egui::DroppedFileHandle>) {
    if files.len() > import_limits::PENDING_LIMIT {
        files.truncate(import_limits::PENDING_LIMIT);
        app.show_error(i18n::tr("Some dropped files were skipped: at most 64 files can be queued at once.").to_string());
    }
}

/// Native disk reads start lazily, with at most two threads running process-wide. The lease must
/// stay with PendingOpen after this future completes: base64 parameters still consume its budget.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dropped_bytes(f: &egui::DroppedFileHandle, name: &str) -> Result<(ReadFuture, import_limits::ImportLease), String> {
    dropped_bytes_limited(f, name, import_limits::Limits::shared())
}

#[cfg(not(target_arch = "wasm32"))]
/// Admits work before retaining it, then starts a disk worker only when a read slot is free.
/// Metadata is checked before opening and before reserving the complete encoding pipeline.
/// Dropping queued work releases admission; started workers retain their own lease until send.
/// The explicit limits also let tests exercise exhaustion without allocating large files.
fn dropped_bytes_limited(
    f: &egui::DroppedFileHandle,
    name: &str,
    limits: Arc<import_limits::Limits>,
) -> Result<(ReadFuture, import_limits::ImportLease), String> {
    use import_limits::FILE_LIMIT;
    let lease = limits.admit().ok_or_else(|| import_busy(name))?;
    let error = unreadable(name);
    let too_large = i18n::trf("Cannot import {name}: file exceeds 512 MB.", &[("name", &name)]);
    let busy = import_busy(name);
    if !f.path().exists() {
        // No size API exists for native virtual handles. Reserve the worst-case raw buffer before
        // asking the backend, then account for the complete encoding/decoding pipeline.
        if !lease.reserve_virtual_read() {
            return Err(busy);
        }
        let bytes = f.bytes().map_err(|_| error)?;
        if bytes.len() > FILE_LIMIT {
            return Err(too_large);
        }
        if !lease.reserve_file(bytes.len()) {
            return Err(busy);
        }
        return Ok((Box::pin(std::future::ready(Ok(bytes))), lease));
    }
    let path = f.path().to_path_buf();
    let roots = figcraft_engine::file_access::current();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let worker_lease = lease.clone();
    let read_error = error.clone();
    let mut job = Some(move || {
        let result = figcraft_engine::file_access::confined(roots.as_ref(), || {
            // Refuse folders/devices/FIFOs before opening, then verify the opened handle too.
            let metadata = figcraft_engine::file_access::metadata(&path).map_err(|_| read_error.clone())?;
            if !metadata.is_file() {
                return Err(read_error);
            }
            if metadata.len() > FILE_LIMIT as u64 {
                return Err(too_large);
            }
            let file = figcraft_engine::file_access::open(&path).map_err(|_| read_error.clone())?;
            let metadata = file.metadata().map_err(|_| read_error.clone())?;
            if !metadata.is_file() {
                return Err(read_error);
            }
            if metadata.len() > FILE_LIMIT as u64 {
                return Err(too_large);
            }
            let size = usize::try_from(metadata.len()).map_err(|_| busy.clone())?;
            if !worker_lease.reserve_file(size) {
                return Err(busy);
            }
            read_sized(file, size).map_err(|e| if e.kind() == std::io::ErrorKind::InvalidData { busy } else { read_error })
        });
        let _ = tx.send(result);
        // Keep worker_lease alive until send completes, even when the UI cancelled the future.
        drop(worker_lease);
    });
    let future = Box::pin(std::future::poll_fn(move |_| {
        if job.is_some() {
            let Some(reader) = limits.reader() else { return Poll::Pending };
            if let Some(job) = job.take() {
                let spawned = std::thread::Builder::new().name("figcraft-drop-read".into()).spawn(move || {
                    let _reader = reader;
                    job();
                });
                if spawned.is_err() {
                    return Poll::Ready(Err(error.clone()));
                }
            }
        }
        match rx.try_recv() {
            Ok(result) => Poll::Ready(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => Poll::Pending,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Poll::Ready(Err(error.clone())),
        }
    }));
    Ok((future, lease))
}

/// Read exactly the reserved metadata length without Vec growth. A file that grows is rejected
/// using a one-byte stack probe; a file that shrinks fails read_exact rather than importing a prefix.
#[cfg(not(target_arch = "wasm32"))]
fn read_sized(mut file: impl std::io::Read, size: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| std::io::Error::other("import allocation failed"))?;
    bytes.resize(size, 0);
    file.read_exact(&mut bytes)?;
    let mut extra = [0];
    loop {
        match file.read(&mut extra) {
            Ok(0) => return Ok(bytes),
            Ok(_) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "file grew beyond its reserved import size")),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Browser drops reserve their known size before starting the asynchronous browser read. Unknown
/// handles reserve the maximum raw size first, just as native virtual files do.
#[cfg(target_arch = "wasm32")]
pub(crate) fn dropped_bytes(f: &egui::DroppedFileHandle, name: &str) -> Result<(ReadFuture, import_limits::ImportLease), String> {
    let limits = import_limits::Limits::shared();
    let lease = limits.admit().ok_or_else(|| import_busy(name))?;
    if let Some(file) = f.web_file() {
        browser_can_read(name, file.size())?;
        if !lease.reserve_file(file.size() as usize) {
            return Err(import_busy(name));
        }
    } else if !lease.reserve_virtual_read() {
        return Err(import_busy(name));
    }
    let (f, name, worker_lease) = (f.clone(), name.to_string(), lease.clone());
    let future = Box::pin(async move {
        let _reader = std::future::poll_fn(move |_| limits.reader().map_or(Poll::Pending, Poll::Ready)).await;
        let bytes = f.bytes_async().await.map_err(|_| unreadable(&name))?;
        if !worker_lease.reserve_file(bytes.len()) {
            return Err(import_busy(&name));
        }
        Ok(bytes)
    });
    Ok((future, lease))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{dropped_bytes_limited, read_sized};
    use crate::import_limits::Limits;
    use std::io::{Cursor, ErrorKind};
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    fn complete(read: &mut crate::ReadFuture) -> Result<Vec<u8>, String> {
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..1000 {
            if let Poll::Ready(result) = read.as_mut().poll(&mut cx) {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Err("reader did not complete".into())
    }

    #[test]
    fn native_completed_results_keep_the_budget_and_failures_release_it() {
        // The current source file supplies stable metadata; no large allocation is needed to
        // exercise aggregate rejection and lazy worker admission.
        #[derive(Debug)]
        struct Disk(std::path::PathBuf);
        impl egui::DroppedFile for Disk {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                panic!("disk reader must use confined file access")
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/open.rs");
        let size = std::fs::metadata(&path).unwrap().len() as usize;
        let file = Arc::new(Disk(path)) as egui::DroppedFileHandle;
        let limits = Arc::new(Limits::new(12 * size + (64 << 10)));
        for _ in 0..3 {
            let readers = (limits.reader().unwrap(), limits.reader().unwrap());
            let (mut first, first_lease) = dropped_bytes_limited(&file, "first.svg", limits.clone()).unwrap();
            assert!(first.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            drop(readers);
            let bytes = complete(&mut first).unwrap();
            let (mut second, second_lease) = dropped_bytes_limited(&file, "second.svg", limits.clone()).unwrap();
            assert!(complete(&mut second).unwrap_err().contains("memory limit"));
            drop((second, second_lease));
            assert_ne!(limits.usage().0, 0, "completed input remains charged");
            drop((first, bytes, first_lease));
            // Result receipt may race the worker's final lease/reader drop.
            for _ in 0..1000 {
                if limits.usage() == (0, 0, 0) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(limits.usage(), (0, 0, 0));
        }
    }

    #[test]
    fn metadata_must_match_the_read_including_empty_files() {
        assert_eq!(read_sized(Cursor::new(b"abc"), 3).unwrap(), b"abc");
        assert!(read_sized(Cursor::new(b""), 0).unwrap().is_empty());
        assert_eq!(read_sized(Cursor::new(b"abcd"), 3).unwrap_err().kind(), ErrorKind::InvalidData);
        assert_eq!(read_sized(Cursor::new(b"a"), 0).unwrap_err().kind(), ErrorKind::InvalidData);
        assert_eq!(read_sized(Cursor::new(b"ab"), 3).unwrap_err().kind(), ErrorKind::UnexpectedEof);
        assert!(read_sized(Cursor::new(b""), usize::MAX).is_err());
    }

    #[test]
    fn failed_reads_propagate() {
        struct Failed;
        impl std::io::Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic read failure"))
            }
        }
        assert!(read_sized(Failed, 3).is_err());
    }

    #[test]
    fn failed_backend_reads_and_cancelled_queued_reads_release_admission() {
        #[derive(Debug)]
        struct Failed(std::path::PathBuf);
        impl egui::DroppedFile for Failed {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                Err("synthetic backend failure".into())
            }
        }
        let limits = Arc::new(Limits::new(1 << 30));
        let virtual_file = Arc::new(Failed(std::path::PathBuf::new())) as egui::DroppedFileHandle;
        assert!(dropped_bytes_limited(&virtual_file, "failed.svg", limits.clone()).is_err());
        assert_eq!(limits.usage(), (0, 0, 0));
        let directory = Arc::new(Failed(env!("CARGO_MANIFEST_DIR").into())) as egui::DroppedFileHandle;
        // Cancellation before polling must not create a worker or retain admission.
        drop(dropped_bytes_limited(&directory, "directory.svg", limits.clone()).unwrap());
        assert_eq!(limits.usage(), (0, 0, 0));
        let (mut read, lease) = dropped_bytes_limited(&directory, "directory.svg", limits.clone()).unwrap();
        assert!(complete(&mut read).is_err());
        drop((read, lease));
        for _ in 0..1000 {
            if limits.usage() == (0, 0, 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(limits.usage(), (0, 0, 0));
    }
}

/// While files are dragged over the window: a tinted, outlined `rect` with `label` in its middle,
/// over everything else, saying what a drop does there.
pub(crate) fn drop_overlay(ctx: &egui::Context, rect: egui::Rect, label: &str) {
    drop_overlay_allowed(ctx, rect, label, true);
}

/// Paints feedback within the receiving region, using the error colour for unsupported drops.
pub(crate) fn drop_overlay_allowed(ctx: &egui::Context, rect: egui::Rect, label: &str, allowed: bool) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let t = theme::Tokens::get(ctx);
    let accent = if allowed { t.accent } else { t.danger };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop_overlay")));
    let area = rect.shrink(4.0);
    painter.rect_filled(area, egui::CornerRadius::same(8), accent.gamma_multiply(0.12));
    painter.rect_stroke(area, egui::CornerRadius::same(8), egui::Stroke::new(2.0, accent), egui::StrokeKind::Inside);
    let galley = painter.layout_no_wrap(label.to_owned(), theme::semibold(14.0), t.text);
    let pill = egui::Rect::from_center_size(area.center(), galley.size() + egui::vec2(32.0, 16.0));
    painter.rect_filled(pill, egui::CornerRadius::same(8), t.panel);
    painter.rect_stroke(pill, egui::CornerRadius::same(8), egui::Stroke::new(1.0, accent), egui::StrokeKind::Inside);
    painter.galley(pill.center() - galley.size() / 2.0, galley, t.text);
    // Hovering sends no pointer events on every platform: keep the overlay current.
    ctx.request_repaint();
}
