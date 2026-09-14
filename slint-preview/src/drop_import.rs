use crate::{Attachment, Note};
use image::{ImageDecoder, ImageReader};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_FILES: usize = 32;
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
#[derive(Debug)]
pub enum Payload {
    Text(String),
    Files(Vec<PathBuf>),
    Image { bytes: Vec<u8>, dib: bool },
}

pub fn decode_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_TEXT_BYTES || bytes.len() % 2 != 0 {
        return Err("文字过大或格式无效".into());
    }
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|c| *c != 0)
        .collect();
    let text = String::from_utf16(&utf16).map_err(|_| "文字编码无效")?;
    if text.trim().is_empty() {
        return Err("没有可收下的文字".into());
    }
    Ok(text)
}
fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits
}
fn batch_name() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

// Only this background import routine reads dropped files. No clipboard reads,
// shell execution, network fetch, move, or source-file deletion is performed.
pub fn import(payload: Payload, dir: &Path) -> Result<Note, String> {
    let mut created = Vec::new();
    let mut directories = Vec::new();
    let result = (|| -> Result<Note, String> {
        let mut note = Note::default();
        match payload {
            Payload::Text(text) => {
                if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                    return Err("文字为空或超过 4 MB".into());
                }
                note.text = text;
            }
            Payload::Files(paths) => {
                if paths.is_empty() || paths.len() > MAX_FILES {
                    return Err("每次可收下 1–32 个文件".into());
                }
                let mut total = 0u64;
                for path in &paths {
                    let metadata = fs::metadata(path).map_err(|_| "无法读取拖入的文件")?;
                    if !metadata.is_file() {
                        return Err("暂不支持文件夹，请拖入具体文件".into());
                    }
                    total = total.checked_add(metadata.len()).ok_or("文件过大")?;
                    if total > MAX_BYTES {
                        return Err("每次文件合计请不超过 100 MB".into());
                    }
                }
                let batch = batch_name();
                let mut copied_total = 0u64;
                for (i, path) in paths.iter().enumerate() {
                    let name = path
                        .file_name()
                        .ok_or("文件名无效")?
                        .to_string_lossy()
                        .into_owned();
                    let mut reader = ImageReader::open(path)
                        .map_err(|e| e.to_string())?
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?;
                    reader.limits(limits());
                    let is_image = reader.format().is_some() && reader.into_dimensions().is_ok();
                    let folder = dir
                        .join(if is_image { "images" } else { "files" })
                        .join(&batch);
                    if !folder.exists() {
                        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                        directories.push(folder.clone());
                    }
                    let stored_name = format!("{i}-{name}");
                    let destination = folder.join(&stored_name);
                    created.push(destination.clone());
                    let mut input = fs::File::open(path)
                        .map_err(|e| e.to_string())?
                        .take(MAX_BYTES - copied_total + 1);
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&destination)
                        .map_err(|e| e.to_string())?;
                    let copied =
                        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
                    copied_total += copied;
                    if copied_total > MAX_BYTES {
                        return Err("文件在复制过程中变大，请重试".into());
                    }
                    let relative = format!("{batch}/{stored_name}");
                    if is_image {
                        note.images.push(relative);
                    } else {
                        note.files.push(Attachment {
                            name,
                            path: relative,
                        });
                    }
                }
            }
            Payload::Image { bytes, dib } => {
                let image = if dib {
                    let mut decoder =
                        image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(bytes))
                            .map_err(|e| e.to_string())?;
                    decoder.set_limits(limits()).map_err(|e| e.to_string())?;
                    image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?
                } else {
                    let mut reader = ImageReader::new(Cursor::new(bytes))
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?;
                    reader.limits(limits());
                    reader.decode().map_err(|e| e.to_string())?
                };
                let filename = format!("drop-{}.png", batch_name());
                fs::create_dir_all(dir.join("images")).map_err(|e| e.to_string())?;
                let target = dir.join("images").join(&filename);
                created.push(target.clone());
                image
                    .save_with_format(target, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                note.images.push(filename);
            }
        }
        Ok(note)
    })();
    if result.is_err() {
        // Remove only files created by this import; never recursively delete.
        for path in created {
            let _ = fs::remove_file(path);
        }
        for path in directories.into_iter().rev() {
            let _ = fs::remove_dir(path);
        }
    }
    result
}

// Only call for an import which failed before its record was saved.
pub fn discard(note: &Note, dir: &Path) {
    for (kind, relative) in note
        .images
        .iter()
        .map(|p| ("images", p))
        .chain(note.files.iter().map(|f| ("files", &f.path)))
    {
        if Path::new(relative)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let base = dir.join(kind);
        let path = base.join(relative);
        if let (Ok(root), Ok(target)) = (base.canonicalize(), path.canonicalize()) {
            if target.starts_with(&root) && target.is_file() {
                let _ = fs::remove_file(&target);
                if let Some(parent) = target.parent() {
                    if parent != root {
                        let _ = fs::remove_dir(parent);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("pocket-drop-tests-{}", batch_name()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let path = self.0.canonicalize().unwrap();
            let temp = std::env::temp_dir().canonicalize().unwrap();
            assert!(path.starts_with(temp));
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pocket-drop-tests-"));
            fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn mixed_batch_keeps_sources_and_distinct_copies() {
        let f = Fixture::new();
        let source = f.0.join("资料.txt");
        fs::write(&source, "中文内容").unwrap();
        let pic = f.0.join("图片.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([240, 150, 60, 255]))
            .save(&pic)
            .unwrap();
        let dir = f.0.join("data");
        let note = import(
            Payload::Files(vec![source.clone(), source.clone(), pic.clone()]),
            &dir,
        )
        .unwrap();
        assert_eq!(note.files.len(), 2);
        assert_eq!(note.images.len(), 1);
        assert_ne!(note.files[0].path, note.files[1].path);
        assert_eq!(
            fs::read_to_string(dir.join("files").join(&note.files[0].path)).unwrap(),
            "中文内容"
        );
        assert!(source.exists() && pic.exists());
        discard(&note, &dir);
        assert!(source.exists() && pic.exists());
        assert_eq!(fs::read_dir(dir.join("files")).unwrap().count(), 0);
    }
    #[test]
    fn unsupported_empty_and_large_inputs_do_not_import() {
        let f = Fixture::new();
        let dir = f.0.join("data");
        assert!(import(Payload::Text("  ".into()), &dir).is_err());
        assert!(import(Payload::Files(vec![f.0.clone()]), &dir).is_err());
        let large = f.0.join("large.bin");
        fs::File::create(&large)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(import(Payload::Files(vec![large]), &dir).is_err());
        assert!(!dir.exists());
        assert!(decode_text(&[1]).is_err());
        assert_eq!(
            decode_text(
                &"中文\0"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            )
            .unwrap(),
            "中文"
        );
    }
    #[test]
    fn direct_dib_and_png_images_decode() {
        let f = Fixture::new();
        let dir = f.0.join("data");
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([30, 100, 200]),
        ));
        let mut bmp = Cursor::new(Vec::new());
        image.write_to(&mut bmp, image::ImageFormat::Bmp).unwrap();
        let note = import(
            Payload::Image {
                bytes: bmp.into_inner()[14..].to_vec(),
                dib: true,
            },
            &dir,
        )
        .unwrap();
        assert_eq!(
            image::open(dir.join("images").join(&note.images[0]))
                .unwrap()
                .width(),
            2
        );
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(import(
            Payload::Image {
                bytes: png.into_inner(),
                dib: false
            },
            &dir
        )
        .is_ok());
        assert!(import(
            Payload::Image {
                bytes: vec![1, 2, 3],
                dib: true
            },
            &dir
        )
        .is_err());
    }
    #[test]
    fn failed_batch_cleans_only_its_own_copies() {
        let f = Fixture::new();
        let dir = f.0.join("data");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("files"), "blocked").unwrap();
        let pic = f.0.join("image.png");
        image::RgbImage::new(2, 2).save(&pic).unwrap();
        let file = f.0.join("note.txt");
        fs::write(&file, "source").unwrap();
        assert!(import(Payload::Files(vec![pic.clone(), file.clone()]), &dir).is_err());
        assert_eq!(fs::read_dir(dir.join("images")).unwrap().count(), 0);
        assert!(pic.exists() && file.exists());
        assert_eq!(fs::read_to_string(dir.join("files")).unwrap(), "blocked");
    }
}
