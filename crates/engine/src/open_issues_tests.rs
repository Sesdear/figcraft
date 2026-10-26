//! Opening a native file that lost something (a damaged image): the session says so, with the
//! reason, and the file still opens.

use figcraft_doc::Document;
use serde_json::json;

use crate::Session;

fn file(name: &str, damage: bool) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("figcraft-open-issues-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Document::new("Pics");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = png.into_inner();
    figcraft_format::add_image(&mut d, png.clone()).unwrap();
    let mut bytes = figcraft_format::save_bytes(&d, None).unwrap();
    if damage {
        // Images are stored as they are: change one byte of the PNG, not its checksum.
        let at = bytes.windows(png.len()).position(|w| w == png).unwrap();
        bytes[at + 30] ^= 0xff;
    }
    let path = dir.join("pics.figcraft");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_file_with_a_damaged_image_opens_and_says_what_was_lost() {
    let path = file("damaged", true);
    let mut s = Session::new();
    let r = s.execute("file.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(r["repairs"].as_array().is_some_and(|a| a.len() == 1), "{r}");
    let m = s.message.take().expect("the session says so");
    assert!(m.starts_with("pics: image ") && m.contains("couldn't be read") && m.contains("drops it"), "{m}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn an_intact_file_opens_without_a_message() {
    let path = file("intact", false);
    let mut s = Session::new();
    let r = s.execute("file.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(r["repairs"].as_array().is_some_and(Vec::is_empty), "{r}");
    assert!(s.message.is_none());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn file_close_rejects_a_bad_index_instead_of_closing_another_document() {
    let mut s = Session::new();
    s.execute("file.new", &json!({})).unwrap();
    s.execute("file.new", &json!({})).unwrap();
    let docs = s.docs.len();
    for index in [json!(-1), json!(1.5), json!(99)] {
        let r = s.execute("file.close", &json!({"index": index, "force": true}));
        assert!(r.is_err(), "index {index} must be rejected, got {r:?}");
        assert_eq!(s.docs.len(), docs, "index {index} closed a document");
    }
}
