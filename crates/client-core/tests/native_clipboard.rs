//! Manual, serialized native test. This changes the real clipboard. It preserves
//! only EasyCopy-supported public formats, not arbitrary application-private data.
#![cfg(any(target_os = "macos", target_os = "windows"))]
use anyhow::{Context, Result};
use easycopy_client_core::{clipboard::NativeClipboard, Clipboard, ClipboardContent};
use std::io::Cursor;

#[test]
#[ignore = "Changes real clipboard; run manually with --ignored --test-threads=1 in a controlled session"]
fn native_text_transparent_png_files_and_sequence_guard() -> Result<()> {
    let clipboard = NativeClipboard::default();
    let previous = clipboard.read()?;
    let mut last_written = clipboard.sequence()?;
    let result = (|| {
        let text = ClipboardContent::Text("EasyCopy 原生验收 🦀\n第二行".into());
        last_written = clipboard
            .write_if_sequence(&text, last_written)?
            .context("剪贴板被外部修改")?;
        assert_eq!(clipboard.read()?, Some(text));
        assert!(clipboard
            .write_if_sequence(
                &ClipboardContent::Text("应拒绝".into()),
                last_written.wrapping_sub(1)
            )?
            .is_none());
        let mut rgba = image::RgbaImage::new(3, 2);
        rgba.put_pixel(0, 0, image::Rgba([255, 24, 8, 0]));
        rgba.put_pixel(1, 0, image::Rgba([5, 230, 20, 128]));
        rgba.put_pixel(2, 0, image::Rgba([0, 10, 255, 255]));
        let mut encoded = Cursor::new(Vec::new());
        rgba.write_to(&mut encoded, image::ImageFormat::Png)?;
        last_written = clipboard
            .write_if_sequence(&ClipboardContent::Image(encoded.into_inner()), last_written)?
            .context("剪贴板被外部修改")?;
        let Some(ClipboardContent::Image(png)) = clipboard.read()? else {
            anyhow::bail!("原生图片格式读回失败");
        };
        let decoded = image::load_from_memory(&png)?.to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
        assert_eq!(decoded.get_pixel(1, 0)[3], 128);
        assert_eq!(decoded.get_pixel(2, 0)[3], 255);
        let temp = tempfile::tempdir()?;
        std::fs::create_dir_all(temp.path().join("项目/嵌套/空目录"))?;
        std::fs::write(temp.path().join("项目/嵌套/文本.txt"), "native files")?;
        std::fs::write(temp.path().join("图片文件.png"), &png)?;
        let paths = vec![temp.path().join("项目"), temp.path().join("图片文件.png")];
        last_written = clipboard
            .write_if_sequence(&ClipboardContent::Files(paths.clone()), last_written)?
            .context("剪贴板被外部修改")?;
        assert_eq!(clipboard.read()?, Some(ClipboardContent::Files(paths)));
        assert!(temp.path().join("项目/嵌套/空目录").is_dir());
        Ok(())
    })();
    if clipboard.sequence()? == last_written {
        if let Some(previous) = previous {
            clipboard.write_if_sequence(&previous, last_written)?;
        } else {
            clipboard.write_if_sequence(&ClipboardContent::Text(String::new()), last_written)?;
        }
    }
    result
}
