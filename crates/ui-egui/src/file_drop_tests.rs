//! OS file gestures go through the same import commands as File ▸ Import.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Event, Pos2, RawInput};
use figcraft_doc::NodeId;
use figcraft_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;
use crate::live_tests::{flat, has_text};

#[derive(Debug)]
struct File(PathBuf, Vec<u8>);

impl egui::DroppedFile for File {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.1.clone())
    }
}

fn svg(name: &str) -> egui::DroppedFileHandle {
    Arc::new(File(name.into(), br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20"/></svg>"#.to_vec()))
}

fn editor() -> Ui {
    let mut u = Ui::new(Session::new());
    u.app.ui.home = false;
    u.frame(vec![]);
    u
}

fn drop_at(u: &mut Ui, pos: Pos2, files: Vec<egui::DroppedFileHandle>) {
    u.frame_raw(RawInput { events: vec![Event::PointerMoved(pos)], dropped_files: files, ..Default::default() });
}

fn finish(u: &mut Ui) {
    for _ in 0..8 {
        u.frame(vec![]);
    }
    assert!(u.app.ui.pending_opens.is_empty());
}

fn children(u: &Ui) -> Vec<NodeId> {
    let ds = u.app.session.ds().unwrap();
    ds.doc.node(ds.page).unwrap().children.clone()
}

#[test]
fn drops_outside_canvas_or_without_a_pointer_do_not_import() {
    let mut u = editor();
    for pos in [Pos2::new(40.0, 250.0), Pos2::new(1380.0, 250.0), Pos2::new(700.0, 12.0)] {
        assert!(!u.app.canvas.rect.contains(pos));
        drop_at(&mut u, pos, vec![svg("outside.svg")]);
        finish(&mut u);
        assert!(children(&u).is_empty());
    }
    u.frame_raw(RawInput { events: vec![Event::PointerGone], dropped_files: vec![svg("unknown.svg")], ..Default::default() });
    finish(&mut u);
    assert!(children(&u).is_empty());
}

#[test]
fn drop_position_uses_the_receiving_view_and_survives_later_pan_and_zoom() {
    for zoom in [0.25, 1.0, 3.0] {
        let mut u = editor();
        let view = &mut u.app.session.ds_mut().unwrap().view;
        view.zoom = zoom;
        view.cx = -725.0;
        view.cy = 348.0;
        let pos = u.app.canvas.rect.center() + egui::vec2(91.0, -57.0);
        let expected = u.app.to_page(pos);
        drop_at(&mut u, pos, vec![svg("position.svg")]);
        let view = &mut u.app.session.ds_mut().unwrap().view;
        view.zoom = 0.5;
        view.cx = 1234.0;
        finish(&mut u);
        let bounds = u.app.session.doc().unwrap().world_bounds(children(&u)[0]).unwrap();
        assert!((bounds.x0 - expected.x).abs() < 0.01, "{bounds:?} vs {expected:?}");
        assert!((bounds.y0 - expected.y).abs() < 0.01);
        assert!((bounds.width() / bounds.height() - 2.0).abs() < 0.01);
    }
}

#[test]
fn a_mixed_drop_keeps_valid_files_and_selects_all_successes() {
    let mut u = editor();
    let pos = u.app.canvas.rect.center();
    drop_at(
        &mut u,
        pos,
        vec![
            svg("a.svg"),
            Arc::new(File("notes.txt".into(), b"notes".to_vec())),
            Arc::new(File("broken.png".into(), b"not an image".to_vec())),
            svg("b.svg"),
        ],
    );
    finish(&mut u);
    let ids = children(&u);
    assert_eq!(ids.len(), 2);
    assert_eq!(u.app.session.selection(), ids);
    assert!(u.app.ui.last_error.is_some(), "the failed import is reported");
    let doc = u.app.session.doc().unwrap();
    assert!(doc.world_bounds(ids[1]).unwrap().x0 > doc.world_bounds(ids[0]).unwrap().x1);
}

#[test]
fn separate_drops_waiting_together_keep_their_own_origins_and_selection() {
    let mut u = editor();
    let first = u.app.canvas.rect.center();
    let second = first + egui::vec2(150.0, 80.0);
    let expected = u.app.to_page(second);
    drop_at(&mut u, first, vec![svg("a.svg"), svg("b.svg")]);
    drop_at(&mut u, second, vec![svg("c.svg"), svg("d.svg")]);
    finish(&mut u);
    let ids = children(&u);
    assert_eq!(ids.len(), 4);
    assert_eq!(u.app.session.selection(), ids[2..]);
    let bounds = u.app.session.doc().unwrap().world_bounds(ids[2]).unwrap();
    assert!((bounds.x0 - expected.x).abs() < 0.01);
    assert!((bounds.y0 - expected.y).abs() < 0.01);
}

#[test]
fn a_discarded_egui_pass_does_not_enqueue_the_drop_twice() {
    let mut u = editor();
    let pos = u.app.canvas.rect.center();
    u.frame_raw_with_discard(RawInput { events: vec![Event::PointerMoved(pos)], dropped_files: vec![svg("once.svg")], ..Default::default() });
    finish(&mut u);
    assert_eq!(children(&u).len(), 1);
}

#[test]
fn an_import_waits_for_an_internal_gesture_and_keeps_its_own_undo_entry() {
    for cancel in [false, true] {
        let mut u = editor();
        let pos = u.app.canvas.rect.center();
        drop_at(&mut u, pos, vec![svg("after-gesture.svg")]);
        u.app.session.begin_interaction("Move existing layer").unwrap();
        for _ in 0..4 {
            u.frame(vec![]);
        }
        assert_eq!(u.app.ui.pending_opens.len(), 1, "ready imports wait for the user's gesture");
        assert!(children(&u).is_empty());
        u.app.session.preview(|_, _| Ok(())).unwrap();
        if cancel {
            u.app.session.cancel_interaction().unwrap();
        } else {
            u.app.session.commit_interaction().unwrap();
        }
        finish(&mut u);
        assert_eq!(children(&u).len(), 1);
        assert!(u.app.session.undo().unwrap());
        assert!(children(&u).is_empty());
        assert!(u.app.session.redo().unwrap());
        assert_eq!(children(&u).len(), 1);
    }
}

#[test]
fn file_hover_is_scoped_to_canvas_and_clears_on_leave_or_cancel() {
    let mut u = editor();
    let hovered = || vec![egui::HoveredFile { path: Some("a.svg".into()), mime: String::new() }];
    let center = u.app.canvas.rect.center();
    for pos in [center, center + egui::vec2(8.0, 6.0)] {
        let out = u.frame_raw(RawInput { events: vec![Event::PointerMoved(pos)], hovered_files: hovered(), ..Default::default() });
        assert!(has_text(&flat(&out.shapes), "Drop to import"));
    }
    let out = u.frame_raw(RawInput { events: vec![Event::PointerMoved(Pos2::new(30.0, 250.0))], hovered_files: hovered(), ..Default::default() });
    assert!(!has_text(&flat(&out.shapes), "Drop to import"));
    let out = u.frame_raw(RawInput { events: vec![Event::PointerMoved(center)], ..Default::default() });
    assert!(!has_text(&flat(&out.shapes), "Drop to import"));
    assert!(children(&u).is_empty());
}

#[test]
fn unsupported_hover_is_distinct_and_an_unsupported_drop_reports_an_error() {
    let mut u = editor();
    let center = u.app.canvas.rect.center();
    let out = u.frame_raw(RawInput {
        events: vec![Event::PointerMoved(center)],
        hovered_files: vec![egui::HoveredFile { path: None, mime: "text/plain".into() }],
        ..Default::default()
    });
    assert!(has_text(&flat(&out.shapes), "Unsupported file type"));
    drop_at(&mut u, center, vec![Arc::new(File("notes.txt".into(), b"notes".to_vec()))]);
    finish(&mut u);
    assert!(children(&u).is_empty());
    assert!(u.app.ui.last_error.as_deref().is_some_and(|e| e.contains("unsupported file type")));
}

#[test]
fn a_damaged_svg_reports_an_error_without_creating_an_object() {
    let mut u = editor();
    let center = u.app.canvas.rect.center();
    drop_at(&mut u, center, vec![Arc::new(File("broken.svg".into(), b"not XML".to_vec()))]);
    finish(&mut u);
    assert!(children(&u).is_empty());
    assert!(u.app.ui.last_error.is_some());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn disk_reads_are_bounded_async_and_keep_the_automation_roots() {
    use figcraft_engine::file_access::{Roots, confined};
    use std::task::{Context, Poll, Waker};

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn read(mut read: crate::ReadFuture) -> Result<Vec<u8>, String> {
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..1000 {
            if let Poll::Ready(result) = read.as_mut().poll(&mut context) {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the file reader never completed");
    }

    let dir = Temp(std::env::temp_dir().join(format!("figcraft-drop-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("small.svg");
    std::fs::write(&path, b"synthetic file").unwrap();
    let file = Arc::new(File(path.clone(), Vec::new())) as egui::DroppedFileHandle;
    assert_eq!(read(crate::dropped_bytes(&file, "small.svg")).unwrap(), b"synthetic file");

    let roots = Arc::new(Roots::new(&[], &[]).unwrap());
    let pending = confined(Some(&roots), || crate::dropped_bytes(&file, "small.svg"));
    assert!(read(pending).is_err(), "the reader cannot escape its captured policy");

    let huge = dir.0.join("huge.png");
    std::fs::File::create(&huge).unwrap().set_len((512 << 20) + 1).unwrap();
    let file = Arc::new(File(huge, Vec::new())) as egui::DroppedFileHandle;
    assert!(read(crate::dropped_bytes(&file, "huge.png")).unwrap_err().contains("512 MB"));
    let file = Arc::new(File(dir.0.clone(), Vec::new())) as egui::DroppedFileHandle;
    assert!(read(crate::dropped_bytes(&file, "folder.png")).is_err(), "folders aren't file contents");
}

#[test]
fn dropped_rasters_are_regular_undoable_and_persisted_image_layers() {
    use image::{DynamicImage, ImageFormat};
    let mut u = editor();
    let mut files = Vec::new();
    for (name, format) in [("a.PNG", ImageFormat::Png), ("b.jpg", ImageFormat::Jpeg), ("c.webp", ImageFormat::WebP)] {
        let image = DynamicImage::new_rgb8(12, 6);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        files.push(Arc::new(File(name.into(), bytes.into_inner())) as egui::DroppedFileHandle);
    }
    let pos = u.app.canvas.rect.center();
    drop_at(&mut u, pos, files);
    finish(&mut u);
    let ids = children(&u);
    assert_eq!(ids.len(), 3);
    assert_eq!(u.app.session.selection(), ids);
    for _ in 0..3 {
        u.app.session.execute("edit.undo", &json!({})).unwrap();
    }
    assert!(children(&u).is_empty());
    for _ in 0..3 {
        u.app.session.execute("edit.redo", &json!({})).unwrap();
    }
    assert_eq!(children(&u), ids);
    let doc = u.app.session.doc().unwrap();
    let saved = figcraft_format::save_bytes(doc, None).unwrap();
    let (loaded, issues) = figcraft_format::load_bytes(&saved).unwrap();
    assert!(issues.is_empty());
    assert_eq!(&loaded, doc);
}
