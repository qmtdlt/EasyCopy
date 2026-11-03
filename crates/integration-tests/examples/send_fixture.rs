//! Sends synthetic data to a local acceptance relay; never reads the system clipboard.
use anyhow::{Context, Result};
use easycopy_client_core::{archive, transport::Transport};
use easycopy_protocol::{ContentKind, Pairing, RegisterItem};
use std::{io::Cursor, path::PathBuf};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pairing_path = PathBuf::from(args.next().context("pairing file required")?);
    let port: u16 = args.next().context("port required")?.parse()?;
    let kind = args.next().unwrap_or_else(|| "text".into());
    let directory = PathBuf::from(args.next().context("fixture directory required")?);
    std::fs::create_dir_all(&directory)?;
    let pairing = Pairing::decode(&std::fs::read_to_string(pairing_path)?)?;
    let transport = Transport::new("127.0.0.1", port, pairing).await?;
    transport.info().await?;
    let (kind, preview) = match kind.as_str() {
        "text" => (
            ContentKind::Text,
            "EasyCopy 实机联调：文本已通过 HTTPS/WSS 到达 Mac。",
        ),
        "image" => (ContentKind::Image, "透明 PNG 实机样本"),
        "files" => (ContentKind::Files, "EasyCopy 验收目录、透明图.png"),
        _ => anyhow::bail!("kind must be text, image or files"),
    };
    // Registration precedes fixture preparation too.
    let item = transport
        .register(&RegisterItem {
            id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            device_name: "本机联调发送端".into(),
            kind,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as i64,
            preview: preview.into(),
        })
        .await?;
    let mut image = image::RgbaImage::new(128, 96);
    for (x, y, p) in image.enumerate_pixels_mut() {
        *p = image::Rgba([
            20,
            180,
            140,
            if (x / 16 + y / 16) % 2 == 0 { 128 } else { 0 },
        ]);
    }
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    let payload = directory.join(format!("{}.payload", item.id));
    match kind {
        ContentKind::Text => std::fs::write(&payload, preview)?,
        ContentKind::Image => std::fs::write(&payload, png.get_ref())?,
        ContentKind::Files => {
            let folder = directory.join("EasyCopy 验收目录");
            std::fs::create_dir_all(folder.join("嵌套/空文件夹"))?;
            std::fs::write(
                folder.join("嵌套/内容.txt"),
                "这是实际传输后供 Finder 粘贴的测试文件。\n",
            )?;
            std::fs::write(directory.join("透明图.png"), png.get_ref())?;
            archive::pack(&[folder, directory.join("透明图.png")], &payload)?;
        }
    }
    let (bytes, hash) = archive::hash_file(&payload)?;
    transport.upload(item.id, &payload, bytes, &hash).await?;
    println!(
        "fixture complete: {:?}, id={}, bytes={bytes}",
        kind, item.id
    );
    Ok(())
}
