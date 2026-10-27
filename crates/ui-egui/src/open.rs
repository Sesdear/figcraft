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

/// Read native drops off the UI thread, carrying the current scoped access policy into the
/// reader. Bound the read itself too: checking metadata alone has a file-growth race.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dropped_bytes(f: &egui::DroppedFileHandle, name: &str) -> ReadFuture {
    use std::io::Read as _;
    const LIMIT: u64 = 512 << 20;
    let error = unreadable(name);
    let too_large = i18n::trf("Cannot import {name}: file exceeds 512 MB.", &[("name", &name)]);
    if !f.path().exists() {
        // Virtual native files already expose their bytes through the backend.
        return Box::pin(std::future::ready(
            f.bytes().map_err(|_| error).and_then(|bytes| if bytes.len() as u64 > LIMIT { Err(too_large) } else { Ok(bytes) }),
        ));
    }
    let path = f.path().to_path_buf();
    let roots = figcraft_engine::file_access::current();
    let (tx, rx) = std::sync::mpsc::channel();
    let read_error = error.clone();
    let spawned = std::thread::Builder::new().name("figcraft-drop-read".into()).spawn(move || {
        let result = figcraft_engine::file_access::confined(roots.as_ref(), || {
            // Refuse folders/devices/FIFOs before opening, then verify the opened handle too.
            if !figcraft_engine::file_access::metadata(&path).map_err(|_| read_error.clone())?.is_file() {
                return Err(read_error);
            }
            let file = figcraft_engine::file_access::open(&path).map_err(|_| read_error.clone())?;
            let metadata = file.metadata().map_err(|_| read_error.clone())?;
            if !metadata.is_file() {
                return Err(read_error);
            }
            if metadata.len() > LIMIT {
                return Err(too_large);
            }
            let mut bytes = Vec::new();
            file.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|_| read_error)?;
            if bytes.len() as u64 > LIMIT { Err(too_large) } else { Ok(bytes) }
        });
        let _ = tx.send(result);
    });
    if spawned.is_err() {
        return Box::pin(std::future::ready(Err(error)));
    }
    Box::pin(std::future::poll_fn(move |_| match rx.try_recv() {
        Ok(result) => Poll::Ready(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => Poll::Pending,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Poll::Ready(Err(error.clone())),
    }))
}

/// Reads the contents of dropped file `name`; the browser hands them over asynchronously.
#[cfg(target_arch = "wasm32")]
pub(crate) fn dropped_bytes(f: &egui::DroppedFileHandle, name: &str) -> ReadFuture {
    let (f, name) = (f.clone(), name.to_string());
    Box::pin(async move {
        if let Some(file) = f.web_file() {
            browser_can_read(&name, file.size())?;
        }
        f.bytes_async().await.map_err(|_| unreadable(&name))
    })
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
