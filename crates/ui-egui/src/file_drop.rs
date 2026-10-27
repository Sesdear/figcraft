//! External file gestures use egui's platform file events, not the editor's object drag state.
use egui::Context;
use serde_json::json;

use crate::FigApp;
use crate::i18n::{tr, trf};
use crate::pending_open::{ImportTarget, PendingOpen};

/// Scope feedback and acceptance to the actual working area. No cached hover flag: leaving,
/// cancellation and a completed drop clear the platform's hovered files automatically.
pub(crate) fn canvas(app: &mut FigApp, ctx: &Context) {
    let pos = ctx.input(|i| i.pointer.hover_pos()).filter(|p| p.is_finite() && app.canvas.rect.contains(*p));
    if pos.is_some() {
        let (documents, supported) = ctx.input(|i| {
            let files = &i.raw.hovered_files;
            (files.iter().all(|f| f.path.as_deref().is_some_and(crate::is_document)), files.iter().any(hover_supported))
        });
        let editable = app.ui.right_tab != crate::RightTab::Inspect;
        let label = if documents {
            tr("Drop to open")
        } else if !editable {
            tr("Dev Mode only inspects: switch to Design (Shift+D) to edit")
        } else if supported {
            tr("Drop to import")
        } else {
            tr("Unsupported file type")
        };
        crate::drop_overlay_allowed(ctx, app.canvas.rect, label, documents || editable && supported);
    }
    // egui may replay RawInput in a discarded layout pass. Only the first pass queues files.
    if ctx.current_pass_index() != 0 {
        return;
    }
    let mut files = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
    let Some(pos) = pos else { return };
    let pt = app.to_page(pos);
    if !pt.x.is_finite() || !pt.y.is_finite() {
        return;
    }
    crate::open::limit_drop(app, &mut files);
    let mut refused = false;
    for file in files {
        let path = file.path();
        if !crate::opens(path) {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            app.show_error(trf("Cannot import {name}: unsupported file type.", &[("name", &name)]));
            continue;
        }
        if crate::is_document(path) {
            crate::pending_open::open_dropped(app, &file);
            continue;
        }
        if refused || crate::menus::dev_mode_refuses(app, "file.import") {
            refused = true;
            continue;
        }
        let Some(target) =
            app.session.ds().ok().map(|ds| ImportTarget { uid: ds.uid, page: ds.page, name: ds.doc.name.clone(), batch: ctx.cumulative_frame_nr() })
        else {
            continue;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let (read, resources) = match crate::dropped_bytes(&file, &name) {
            Ok(read) => read,
            Err(error) => {
                app.show_error(error);
                continue;
            }
        };
        app.ui.pending_opens.push(PendingOpen {
            label: name.clone(),
            params: json!({"name": name, "x": pt.x, "y": pt.y}),
            command: "file.import",
            target: Some(target),
            read: Some(read),
            resources: Some(resources),
        });
    }
}

/// Checks hover metadata, accepting inconclusive browser MIME types until the dropped file can
/// be checked by name and parsed. Browser drags do not expose names until release.
fn hover_supported(file: &egui::HoveredFile) -> bool {
    if let Some(path) = &file.path {
        return crate::opens(path);
    }
    // Browsers expose MIME, not names, until drop. An empty MIME is inconclusive (Safari, SVG
    // and native documents); the actual file is still checked by name and by the import parser.
    matches!(
        file.mime.as_str(),
        "" | "image/png"
            | "image/jpeg"
            | "image/webp"
            | "image/svg+xml"
            | "image/gif"
            | "image/bmp"
            | "image/x-ms-bmp"
            | "image/tiff"
            | "application/json"
            | "application/octet-stream"
    )
}
