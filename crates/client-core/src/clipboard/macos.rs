use super::{Clipboard, ClipboardContent};
use anyhow::{bail, Context, Result};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};
use std::{io::Cursor, path::PathBuf};

fn read_files(board: &NSPasteboard) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if let Some(items) = board.pasteboardItems() {
        for item in items.iter() {
            if let Some(value) = item.stringForType(&NSString::from_str("public.file-url")) {
                // Finder may supply file:///.file/id=… references. A generic URL
                // parser treats those as literal paths; Foundation resolves the
                // file identity to its current path (including after a rename).
                let native = NSURL::URLWithString(&value).context("剪贴板文件 URL 无效")?;
                if !native.isFileURL() {
                    bail!("剪贴板文件 URL 无效");
                }
                let resolved = native
                    .filePathURL()
                    .context("无法定位复制的文件，文件可能已删除或所在磁盘已断开")?;
                let value = resolved.absoluteString().context("剪贴板文件路径无效")?;
                let url = url::Url::parse(&value.to_string())?;
                files.push(
                    url.to_file_path()
                        .map_err(|_| anyhow::anyhow!("剪贴板文件 URL 无效"))?,
                );
            }
        }
    }
    Ok(files)
}

#[derive(Default)]
pub struct NativeClipboard;
impl Clipboard for NativeClipboard {
    fn sequence(&self) -> Result<u64> {
        Ok(NSPasteboard::generalPasteboard().changeCount() as u64)
    }
    fn read(&self) -> Result<Option<ClipboardContent>> {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            // Finder file references take priority over their image/text representations.
            let files = read_files(&board)?;
            if !files.is_empty() {
                return Ok(Some(ClipboardContent::Files(files)));
            }
            if let Some(data) = board.dataForType(&NSString::from_str("public.png")) {
                return Ok(Some(ClipboardContent::Image(data.to_vec())));
            }
            if let Some(data) = board.dataForType(&NSString::from_str("public.tiff")) {
                let image =
                    image::load_from_memory_with_format(&data.to_vec(), image::ImageFormat::Tiff)?;
                let mut png = Cursor::new(Vec::new());
                image.write_to(&mut png, image::ImageFormat::Png)?;
                return Ok(Some(ClipboardContent::Image(png.into_inner())));
            }
            Ok(board
                .stringForType(&NSString::from_str("public.utf8-plain-text"))
                .map(|s| ClipboardContent::Text(s.to_string())))
        })
    }
    fn write_if_sequence(&self, content: &ClipboardContent, expected: u64) -> Result<Option<u64>> {
        objc2::rc::autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            let mut items = Vec::new();
            match content {
                ClipboardContent::Files(paths) => {
                    for path in paths {
                        if !path.exists() {
                            bail!("缓存文件不存在：{}", path.display());
                        }
                        let url = url::Url::from_file_path(path)
                            .map_err(|_| anyhow::anyhow!("缓存文件路径无效"))?;
                        let item = NSPasteboardItem::new();
                        if !item.setString_forType(
                            &NSString::from_str(url.as_str()),
                            &NSString::from_str("public.file-url"),
                        ) {
                            bail!("无法生成原生文件引用");
                        }
                        items.push(item);
                    }
                }
                ClipboardContent::Text(text) => {
                    let item = NSPasteboardItem::new();
                    if !item.setString_forType(
                        &NSString::from_str(text),
                        &NSString::from_str("public.utf8-plain-text"),
                    ) {
                        bail!("无法写入文本");
                    }
                    items.push(item);
                }
                ClipboardContent::Image(bytes) => {
                    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                        .context("图片 PNG 无效")?;
                    let item = NSPasteboardItem::new();
                    if !item.setData_forType(
                        &NSData::with_bytes(bytes),
                        &NSString::from_str("public.png"),
                    ) {
                        bail!("无法写入图片");
                    }
                    items.push(item);
                }
            }
            let marker = uuid::Uuid::new_v4().to_string();
            let marker_type = NSString::from_str("app.easycopy.write-marker");
            for item in &items {
                if !item.setString_forType(&NSString::from_str(&marker), &marker_type) {
                    bail!("无法写入剪贴板来源标记");
                }
            }
            let objects: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = items
                .iter()
                .map(|i| ProtocolObject::from_ref(&**i))
                .collect();
            let objects = NSArray::from_slice(&objects);
            if board.changeCount() as u64 != expected {
                return Ok(None);
            }
            board.clearContents();
            if !board.writeObjects(&objects) {
                bail!("系统拒绝写入剪贴板");
            }
            let written = board.changeCount() as u64;
            let own_marker = board.stringForType(&marker_type).map(|s| s.to_string());
            if board.changeCount() as u64 != written
                || own_marker.as_deref() != Some(marker.as_str())
            {
                return Ok(None);
            }
            Ok(Some(written))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    fn reference_url(path: &Path) -> String {
        let value = url::Url::from_file_path(path).unwrap();
        let native = NSURL::URLWithString(&NSString::from_str(value.as_str())).unwrap();
        let reference = native.fileReferenceURL().unwrap();
        assert!(reference.isFileReferenceURL());
        reference.absoluteString().unwrap().to_string()
    }

    // Use an isolated native pasteboard; never read or replace the user's clipboard.
    fn read_urls(values: &[String]) -> Result<Vec<PathBuf>> {
        let board = NSPasteboard::pasteboardWithUniqueName();
        let items: Vec<_> = values
            .iter()
            .map(|value| {
                let item = NSPasteboardItem::new();
                assert!(item.setString_forType(
                    &NSString::from_str(value),
                    &NSString::from_str("public.file-url"),
                ));
                item
            })
            .collect();
        let objects: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = items
            .iter()
            .map(|item| ProtocolObject::from_ref(&**item))
            .collect();
        assert!(board.writeObjects(&NSArray::from_slice(&objects)));
        let result = read_files(&board);
        board.clearContents();
        result
    }

    #[test]
    fn file_references_resolve_before_archive() -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let temp = tempfile::tempdir()?;
            let root = temp.path().canonicalize()?;
            let folder = root.join("目录 空格");
            let file = root.join("中文 #百分号%;.txt");
            fs::create_dir_all(folder.join("嵌套/空目录"))?;
            fs::write(folder.join("嵌套/内容.txt"), "目录内容")?;
            fs::write(&file, "文件内容")?;
            let urls = vec![reference_url(&folder), reference_url(&file)];
            assert!(urls.iter().all(|value| value.contains("/.file/id=")));
            let paths = read_urls(&urls)?;
            assert_eq!(paths, vec![folder, file]);
            let payload = root.join("payload.tar");
            crate::archive::pack(&paths, &payload)?;
            let destination = root.join("received");
            crate::archive::unpack(&payload, &destination)?;
            assert_eq!(
                fs::read(destination.join("中文 #百分号%;.txt"))?,
                "文件内容".as_bytes()
            );
            assert_eq!(
                fs::read(destination.join("目录 空格/嵌套/内容.txt"))?,
                "目录内容".as_bytes()
            );
            assert!(destination.join("目录 空格/嵌套/空目录").is_dir());
            Ok(())
        })
    }

    #[test]
    fn file_reference_tracks_rename() -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let temp = tempfile::tempdir()?;
            let root = temp.path().canonicalize()?;
            let old = root.join("旧名称.txt");
            let new = root.join("新名称.txt");
            fs::write(&old, "same inode")?;
            let reference = reference_url(&old);
            fs::rename(&old, &new)?;
            assert_eq!(read_urls(&[reference])?, vec![new]);
            Ok(())
        })
    }

    #[test]
    fn ordinary_file_url_preserves_escaped_name() -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let temp = tempfile::tempdir()?;
            let path = temp.path().canonicalize()?.join("中文 空格#%;.txt");
            fs::write(&path, "ordinary URL")?;
            let url = url::Url::from_file_path(&path).unwrap().to_string();
            assert_eq!(read_urls(&[url])?, vec![path]);
            assert!(read_urls(&["https://example.com/file.txt".into()]).is_err());
            Ok(())
        })
    }

    #[test]
    fn deleted_file_reference_is_rejected() -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let temp = tempfile::tempdir()?;
            let path = temp.path().canonicalize()?.join("deleted.txt");
            fs::write(&path, "gone")?;
            let reference = reference_url(&path);
            fs::remove_file(&path)?;
            assert!(read_urls(&[reference]).is_err());
            Ok(())
        })
    }
}
