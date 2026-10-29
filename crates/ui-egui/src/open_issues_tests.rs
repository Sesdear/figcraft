//! Opening a native file with a damaged image: the app says what was lost.

use base64::Engine as _;
use figcraft_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;

#[test]
fn opening_a_file_with_a_damaged_image_toasts_what_was_lost() {
    let dir = std::env::temp_dir().join(format!("figcraft-ui-open-issues-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pics.figcraft");
    let mut s = Session::new();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = png.into_inner();
    let data = base64::engine::general_purpose::STANDARD.encode(&png);
    s.execute("file.import", &json!({"data": data, "name": "dot.png"})).unwrap();
    s.execute("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    // Images are stored as they are: change one byte of the PNG, not its checksum.
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes.windows(png.len()).position(|w| w == png).unwrap();
    bytes[at + 30] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();

    let mut u = Ui::new(Session::new());
    u.app.run("file.open", json!({"path": path.to_string_lossy()}));
    u.frame(vec![]);
    let toast = u.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.starts_with("pics: image ") && toast.contains("couldn't be read"), "{toast}");
    let _ = std::fs::remove_dir_all(&dir);
}
