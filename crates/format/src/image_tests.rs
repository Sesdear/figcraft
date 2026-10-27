//! Image imports validate pixels before registration; damaged or missing native-file members
//! are reported, not dropped without a word.

use std::io::Write;

use figcraft_doc::{Document, NodeKind};

use super::*;

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 0, 0, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn doc_with_image() -> (Document, String) {
    let mut d = Document::new("File");
    let p = d.first_page().unwrap();
    d.create(p, NodeKind::Rectangle, "R", 0.0, 0.0, 10.0, 10.0).unwrap();
    let (hash, ..) = add_image(&mut d, png()).unwrap();
    (d, hash)
}

/// Change just a synthetic PNG's IHDR, including its checksum, without allocating its pixels.
fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png();
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    let mut crc = !0u32;
    for byte in &bytes[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 0 { 0 } else { 0xedb8_8320 };
        }
    }
    bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
    bytes
}

#[test]
fn image_import_preserves_each_supported_raster_format() {
    use image::ImageFormat;
    let mut doc = Document::new("Images");
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 3, image::Rgb([9, 5, 1])));
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Gif, ImageFormat::Bmp, ImageFormat::Tiff] {
        let mut bytes = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut bytes), format).unwrap();
        let (hash, width, height) = add_image(&mut doc, bytes.clone()).unwrap();
        assert_eq!((width, height), (2, 3), "{format:?}");
        assert_eq!(hash, image_hash(&bytes), "{format:?}");
        let stored = doc.images.get(&hash).unwrap();
        assert_eq!((stored.width, stored.height), (2, 3), "{format:?}");
        assert_eq!(stored.bytes.as_slice(), bytes, "{format:?}");
    }
}

#[test]
fn image_import_rejects_a_valid_header_with_corrupt_pixels_without_mutation() {
    let (mut doc, original_hash) = doc_with_image();
    let mut bytes = png();
    let data = bytes.windows(4).position(|window| window == b"IDAT").unwrap() + 4;
    bytes[data] ^= 0xff;
    let dimensions = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().unwrap().into_dimensions().unwrap();
    assert_eq!(dimensions, (2, 2), "a metadata-only check still accepts this image");
    assert!(add_image(&mut doc, bytes).is_err());
    assert_eq!(doc.images.len(), 1);
    assert!(doc.images.contains_key(&original_hash));
}

#[test]
fn image_import_rejects_zero_dimensions() {
    let mut doc = Document::new("Images");
    for (width, height) in [(0, 0), (0, 2), (2, 0)] {
        assert!(add_image(&mut doc, png_with_dimensions(width, height)).is_err(), "{width}×{height}");
        assert!(doc.images.is_empty());
    }
}

#[test]
fn image_import_rejects_oversized_pixels_from_the_header() {
    let mut doc = Document::new("Images");
    // RGBA8 at this size takes 1 GiB. The tiny fixture keeps its original 2×2-pixel payload:
    // the size limit must reject it before decoding or allocating that claimed pixel buffer.
    let error = add_image(&mut doc, png_with_dimensions(16_384, 16_384)).unwrap_err().to_string();
    assert!(error.contains("too large") || error.contains("limit"), "{error}");
    assert!(doc.images.is_empty());
}

/// The archive of `d` rebuilt with `edit` applied to each member (None drops it).
fn rebuild(bytes: &[u8], edit: impl Fn(&str, Vec<u8>) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(&mut buf);
    for n in names {
        let data = read_member(&mut z, &n, u64::MAX).unwrap();
        if let Some(data) = edit(&n, data) {
            w.start_file(n, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            w.write_all(&data).unwrap();
        }
    }
    w.finish().unwrap();
    buf.into_inner()
}

#[test]
fn an_intact_file_loads_without_image_issues() {
    let (d, hash) = doc_with_image();
    let (loaded, issues) = load_bytes(&save_bytes(&d, None).unwrap()).unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    assert!(loaded.images.contains_key(&hash));
}

#[test]
fn a_missing_image_member_is_reported() {
    let (d, hash) = doc_with_image();
    let bytes = rebuild(&save_bytes(&d, None).unwrap(), |n, data| (!n.starts_with("images/")).then_some(data));
    let (loaded, issues) = load_bytes(&bytes).unwrap();
    assert!(!loaded.images.contains_key(&hash));
    assert!(issues.iter().any(|i| i.contains(&hash) && i.contains("missing") && i.contains("drops it")), "{issues:?}");
}

#[test]
fn a_damaged_image_member_is_reported_with_its_reason() {
    let (d, hash) = doc_with_image();
    let bytes = save_bytes(&d, None).unwrap();
    // The member's bytes change but not its checksum: reading it fails.
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let name = format!("images/{hash}");
    let stored = read_member(&mut z, &name, u64::MAX).unwrap();
    let at = bytes.windows(stored.len()).position(|w| w == stored).unwrap();
    let mut damaged = bytes.clone();
    damaged[at + stored.len() / 2] ^= 0xff;
    let (loaded, issues) = load_bytes(&damaged).unwrap();
    assert!(!loaded.images.contains_key(&hash));
    assert!(issues.iter().any(|i| i.contains(&hash) && i.contains("couldn't be read") && i.contains("drops it")), "{issues:?}");
}

#[test]
fn many_lost_images_are_counted_not_listed_one_by_one() {
    let mut d = Document::new("File");
    for k in 0..40u8 {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([k, 1, 0, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        add_image(&mut d, out).unwrap();
    }
    let bytes = rebuild(&save_bytes(&d, None).unwrap(), |n, data| (!n.starts_with("images/")).then_some(data));
    let (_, issues) = load_bytes(&bytes).unwrap();
    assert_eq!(issues.len(), MAX_IMAGE_ISSUES + 1, "{issues:?}");
    assert_eq!(issues.last().map(String::as_str), Some("20 more images couldn't be loaded"));
}

#[test]
fn image_members_with_path_like_names_are_dropped_and_not_saved_back() {
    let (d, hash) = doc_with_image();
    let bytes = save_bytes(&d, None).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let doc_json = read_member(&mut z, "document.json", u64::MAX).unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(&mut buf);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("document.json", opts).unwrap();
    w.write_all(&doc_json).unwrap();
    for n in ["images/../../x", "images/a/b"] {
        w.start_file(n, opts).unwrap();
        w.write_all(&png()).unwrap();
    }
    w.finish().unwrap();
    let hostile = buf.into_inner();
    let loaded = load_bytes_with_versions(&hostile).unwrap();
    assert!(loaded.doc.images.keys().all(|k| k != "../../x" && k != "a/b"), "{:?}", loaded.doc.images.keys());
    assert!(!loaded.issues.is_empty());
    let saved = save_bytes_with_history(&loaded.doc, None, &loaded.versions, &loaded.branches).unwrap();
    let z = zip::ZipArchive::new(std::io::Cursor::new(saved)).unwrap();
    assert!(z.file_names().all(|n| !n.contains("..") && n.matches('/').count() <= 1), "{:?}", z.file_names().collect::<Vec<_>>());
    let _ = hash;
}
