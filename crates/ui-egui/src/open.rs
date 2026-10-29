#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
use std::task::Poll;

use serde_json::{Value, json};

#[cfg(target_arch = "wasm32")]
use super::services::browser_can_read;
use super::services::{ReadFuture, unreadable};
use super::{FigApp, OsStr, Path, i18n, import_limits, theme};

/// The file types File ▸ Open and a drop on the home screen open.
pub(crate) const OPEN_EXTENSIONS: &[&str] =
    &["figcraft", "fig", "penpot", "sketch", "json", "svg", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

/// Whether `path` names a file type that opens as a document (the extension, in any case).
pub(crate) fn opens(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|e| OPEN_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Whether `path` is a whole document (`.figcraft`, `.fig`, `.penpot`, `.sketch`, file JSON) rather than an image or SVG:
/// dropped on the canvas it opens as a file of its own instead of adding its pages to the open one.
pub(crate) fn is_document(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|e| ["figcraft", "fig", "penpot", "sketch", "json"].iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Admission failures share the drop error path; no future, handle or thread is retained.
pub(crate) fn import_busy(name: &str) -> String {
    i18n::trf("Cannot import {name}: too many pending files or import memory limit exceeded.", &[("name", &name)])
}

/// Bound per-gesture processing and error toasts as well as the admitted pending queue.
pub(crate) fn limit_drop(app: &mut FigApp, files: &mut Vec<egui::DroppedFileHandle>) {
    release_replaced_import(app);
    if files.len() > import_limits::PENDING_LIMIT {
        files.truncate(import_limits::PENDING_LIMIT);
        app.show_error(i18n::tr("Some dropped files were skipped: at most 64 files can be queued at once.").to_string());
    }
}

/// Direct engine calls can replace Repeat without going through FigApp::run. Refund the old
/// reservation before another drop is admitted. Batches are kept conservatively because they
/// can contain an import with retained bytes of their own.
pub(crate) fn release_replaced_import(app: &mut FigApp) {
    let retains_input =
        app.session.last_command.as_ref().is_some_and(|(id, params)| id == "command.batch" || (id == "file.import" && params.get("data").is_some()));
    if !retains_input {
        app.repeat_import_memory = None;
    }
}

/// Keep native files as paths so the engine reads them directly without a UI base64 copy or the
/// byte-drop budget. Only regular files pass admission; the engine rechecks access at execution.
/// Browser and native virtual handles retain the bounded asynchronous byte-import pipeline.
pub(crate) fn dropped_source(f: &egui::DroppedFileHandle, name: &str) -> Result<(Value, Option<ReadFuture>, import_limits::ImportLease), String> {
    #[cfg(not(target_arch = "wasm32"))]
    if f.path().exists() {
        let metadata = figcraft_engine::file_access::metadata(f.path()).map_err(|_| unreadable(name))?;
        if !metadata.is_file() {
            return Err(unreadable(name));
        }
        let lease = import_limits::Limits::shared().admit().ok_or_else(|| import_busy(name))?;
        return Ok((json!({"path": f.path().to_string_lossy()}), None, lease));
    }
    dropped_bytes(f, name).map(|(read, lease)| (json!({"name": name}), Some(read), lease))
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
    use super::{dropped_bytes_limited, dropped_source, read_sized};
    use crate::import_limits::Limits;
    use std::io::{Cursor, ErrorKind, Write};
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    /// Native handles must not read or clone bytes when the engine can use their disk path.
    #[derive(Debug)]
    struct PathOnly(std::path::PathBuf);

    impl egui::DroppedFile for PathOnly {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("path imports must not ask the backend for bytes".into())
        }
    }

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn new() -> std::io::Result<Self> {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("figcraft-path-drop-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn large_native_sources_keep_paths_without_byte_reads_or_byte_limits() {
        let dir = Temp::new().unwrap();
        for size in [86u64 << 20, 513u64 << 20] {
            let path = dir.0.join(format!("{size}.figcraft"));
            std::fs::File::create(&path).unwrap().set_len(size).unwrap();
            let handle = Arc::new(PathOnly(path.clone())) as egui::DroppedFileHandle;
            let (params, read, _lease) = dropped_source(&handle, "large.figcraft").unwrap();
            assert_eq!(params, serde_json::json!({"path": path.to_string_lossy()}));
            assert!(read.is_none(), "native paths must not create byte-read work");
            assert!(params.get("data").is_none());
        }
    }

    #[test]
    fn an_image_above_the_byte_drop_budget_imports_by_path_and_remains_undoable() {
        let dir = Temp::new().unwrap();
        let path = dir.0.join("large.png");
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(12, 6).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let size = 86u64 << 20;
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&png).unwrap();
        // PNG ends at IEND; trailing zeroes make the encoded file exceed the old approximately
        // 85 MiB byte-pipeline limit without a large fixture or large decoded pixel allocation.
        file.set_len(size).unwrap();
        drop(file);
        let handle = Arc::new(PathOnly(path)) as egui::DroppedFileHandle;
        let (params, read, _lease) = dropped_source(&handle, "large.png").unwrap();
        assert!(read.is_none());
        let mut session = figcraft_engine::Session::new();
        let result = session.execute("file.import", &params).unwrap();
        let id = figcraft_doc::NodeId(result["id"].as_u64().unwrap());
        let doc = session.doc().unwrap();
        assert!(doc.node(id).is_some());
        assert_eq!(doc.images.len(), 1);
        let image = doc.images.values().next().unwrap();
        assert_eq!((image.width, image.height), (12, 6));
        assert_eq!(image.bytes.len() as u64, size);
        assert!(image.bytes.starts_with(&png));
        session.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert!(session.doc().unwrap().node(id).is_none());
        session.execute("edit.redo", &serde_json::json!({})).unwrap();
        assert!(session.doc().unwrap().node(id).is_some());
    }

    #[test]
    fn native_path_sources_import_real_files_and_reject_nonfiles_or_denied_paths() {
        use figcraft_engine::file_access::{Roots, confined};
        let dir = Temp::new().unwrap();
        let path = dir.0.join("small.svg");
        std::fs::write(&path, br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20"/></svg>"#).unwrap();
        let handle = Arc::new(PathOnly(path)) as egui::DroppedFileHandle;
        let (params, read, _lease) = dropped_source(&handle, "small.svg").unwrap();
        assert!(read.is_none());
        let mut session = figcraft_engine::Session::new();
        let result = session.execute("file.import", &params).unwrap();
        assert!(result.get("id").is_some(), "{result}");
        session.execute("edit.undo", &serde_json::json!({})).unwrap();
        session.execute("edit.redo", &serde_json::json!({})).unwrap();

        let roots = Arc::new(Roots::new(&[], &[]).unwrap());
        assert!(confined(Some(&roots), || dropped_source(&handle, "small.svg")).is_err());
        let directory = Arc::new(PathOnly(dir.0.clone())) as egui::DroppedFileHandle;
        assert!(dropped_source(&directory, "directory.svg").is_err());
    }

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
