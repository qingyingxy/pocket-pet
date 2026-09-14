use image::ImageDecoder;
use std::io::Cursor;
use windows::Win32::{
    Foundation::HGLOBAL,
    System::{DataExchange::*, Memory::*},
};
struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn decode(bytes: Vec<u8>) -> Result<image::RgbaImage, String> {
    let mut decoder = image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(bytes))
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    decoder.set_limits(limits).map_err(|e| e.to_string())?;
    Ok(image::DynamicImage::from_decoder(decoder)
        .map_err(|e| e.to_string())?
        .to_rgba8())
}

// Clipboard owns these handles: unlock them, never free them.
pub fn read_image() -> Result<Option<image::RgbaImage>, String> {
    let original = match arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
        Ok(i) => {
            return image::RgbaImage::from_raw(
                i.width as u32,
                i.height as u32,
                i.bytes.into_owned(),
            )
            .map(Some)
            .ok_or_else(|| "剪贴板图片尺寸无效".into())
        }
        Err(e) => e,
    };
    unsafe {
        OpenClipboard(None).map_err(|_| "剪贴板正忙，请稍后再粘贴")?;
        let _guard = ClipboardGuard;
        let mut errors = Vec::new();
        for format in [17, 8] {
            if IsClipboardFormatAvailable(format).is_err() {
                continue;
            }
            let result = (|| {
                let handle = HGLOBAL(GetClipboardData(format).map_err(|e| e.to_string())?.0);
                let size = GlobalSize(handle);
                if size == 0 || size > 64 * 1024 * 1024 {
                    return Err("剪贴板图片为空或超过 64 MB".into());
                }
                let ptr = GlobalLock(handle);
                if ptr.is_null() {
                    return Err("无法读取剪贴板图片".into());
                }
                let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
                let _ = GlobalUnlock(handle);
                decode(bytes)
            })();
            match result {
                Ok(i) => return Ok(Some(i)),
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            return Err(format!("图片粘贴失败：{}", errors.join("；")));
        }
    }
    match original {
        arboard::Error::ContentNotAvailable => Ok(None),
        e => Err(format!("图片粘贴失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dib(header: usize, bitfields: bool) -> Vec<u8> {
        let offset = header + if bitfields && header == 40 { 12 } else { 0 };
        let mut bytes = vec![0u8; offset + 4];
        bytes[0..4].copy_from_slice(&(header as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&1i32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1i32.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[16..20].copy_from_slice(&(if bitfields { 3u32 } else { 0 }).to_le_bytes());
        if bitfields {
            for (i, mask) in [0xff0000u32, 0xff00, 0xff].iter().enumerate() {
                bytes[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
            }
        }
        bytes[offset..].copy_from_slice(&[30, 20, 10, 0]);
        bytes
    }
    #[test]
    fn pixpin_dib_bitfields() {
        let i = decode(dib(40, true)).unwrap();
        assert_eq!(i.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }
    #[test]
    fn pixpin_v5_rgb_ignores_unused_alpha() {
        let i = decode(dib(124, false)).unwrap();
        assert_eq!(i.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }
    #[test]
    fn truncated_bitmap_reports_error() {
        assert!(decode(vec![0; 12]).is_err());
        assert!(decode(dib(40, true)[..52].to_vec()).is_err());
    }
}
