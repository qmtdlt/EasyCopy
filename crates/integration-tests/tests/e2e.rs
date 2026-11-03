use anyhow::{bail, Result};
use easycopy_client_core::{
    clipboard::{ClipboardContent, MemoryClipboard},
    transport::Transport,
    ClientCore, ClientEvent, SettingsInput,
};
use easycopy_protocol::{ContentKind, ItemStatus, Pairing, RegisterItem};
use easycopy_relay::{serve_relay, Relay};
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    net::{SocketAddr, TcpListener},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

async fn until(description: &str, condition: impl Fn() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        if Instant::now() > deadline {
            bail!("等待超时：{description}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}
fn input(pairing: &Pairing, port: u16) -> SettingsInput {
    SettingsInput {
        address: "127.0.0.1".into(),
        port,
        pairing_key: pairing.encode().unwrap(),
    }
}
fn intent(kind: ContentKind, preview: &str) -> RegisterItem {
    RegisterItem {
        id: Uuid::new_v4(),
        device_id: Uuid::new_v4(),
        device_name: "integration peer".into(),
        kind,
        created_at: 0,
        preview: preview.into(),
    }
}
fn transparent_png() -> Vec<u8> {
    let mut image = image::RgbaImage::new(4, 3);
    image.put_pixel(0, 0, image::Rgba([255, 20, 30, 0]));
    image.put_pixel(1, 0, image::Rgba([20, 240, 30, 128]));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn digest(path: &Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn real_tls_two_way_sync_ordering_files_and_offline_snapshot() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let server = tokio::spawn(serve_relay(relay.clone(), address));
    let port = address.port();
    let wire = Transport::new("127.0.0.1", port, pairing.clone()).await?;
    for attempt in 0..40 {
        if wire.info().await.is_ok() {
            break;
        }
        if attempt == 39 {
            bail!("服务未启动");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let clip_a = Arc::new(MemoryClipboard::default());
    let clip_b = Arc::new(MemoryClipboard::default());
    let a = ClientCore::open_with_clipboard(temp.path().join("a"), clip_a.clone())?;
    let b = ClientCore::open_with_clipboard(temp.path().join("b"), clip_b.clone())?;
    let observations = Arc::new(Mutex::new(Vec::new()));
    let mut receivers = vec![a.subscribe(), b.subscribe()];
    let mut watchers = Vec::new();
    for mut receiver in receivers.drain(..) {
        let observations = observations.clone();
        watchers.push(tokio::spawn(async move {
            while let Ok(event) = receiver.recv().await {
                if let ClientEvent::Transfer(event) = event {
                    observations.lock().unwrap().push(event);
                }
            }
        }));
    }
    a.save_settings(input(&pairing, port)).await?;
    b.save_settings(input(&pairing, port)).await?;
    a.start().await;
    b.start().await;
    until("两客户端 WS 快照", || {
        a.is_connected() && b.is_connected()
    })
    .await?;

    let first = a
        .submit_content(ClipboardContent::Text("从 A 到 B：你好 🦀\nUTF-8".into()))
        .await?;
    until("A → B 文本", || {
        clip_b.content() == Some(ClipboardContent::Text("从 A 到 B：你好 🦀\nUTF-8".into()))
    })
    .await?;
    b.submit_content(ClipboardContent::Text("B → A".into()))
        .await?;
    until("B → A 文本", || {
        clip_a.content() == Some(ClipboardContent::Text("B → A".into()))
    })
    .await?;
    let png = transparent_png();
    a.submit_content(ClipboardContent::Image(png.clone()))
        .await?;
    until("透明 PNG", || {
        clip_b.content() == Some(ClipboardContent::Image(png.clone()))
    })
    .await?;
    b.submit_content(ClipboardContent::Image(png.clone()))
        .await?;
    until("透明 PNG 反向历史", || {
        a.history().first().is_some_and(|i| {
            i.kind == ContentKind::Image
                && i.id != first
                && i.device_name.len() > 0
                && i.status == "complete"
                && i.id == b.history()[0].id
        })
    })
    .await?;

    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("工程/嵌套/空目录"))?;
    std::fs::write(source.join("工程/嵌套/内容.txt"), "文件内容\n")?;
    std::fs::write(source.join("透明图.png"), &png)?;
    let roots = vec![source.join("工程"), source.join("透明图.png")];
    let files_id = a
        .submit_content(ClipboardContent::Files(roots.clone()))
        .await?;
    until("多文件与嵌套/空目录", || {
        b.history()
            .iter()
            .any(|i| i.id == files_id && i.status == "complete")
    })
    .await?;
    let Some(ClipboardContent::Files(received)) = clip_b.content() else {
        bail!("接收文件没有写入文件引用");
    };
    assert!(received
        .iter()
        .all(|p| p.starts_with(temp.path().join("b/cache"))));
    let folder = received
        .iter()
        .find(|p| p.file_name().unwrap() == "工程")
        .unwrap();
    assert!(folder.join("嵌套/空目录").is_dir());
    assert_eq!(
        digest(&folder.join("嵌套/内容.txt")),
        digest(&roots[0].join("嵌套/内容.txt"))
    );
    assert_eq!(
        digest(
            received
                .iter()
                .find(|p| p.file_name().unwrap() == "透明图.png")
                .unwrap()
        ),
        digest(&roots[1])
    );
    let saved = temp.path().join("saved");
    std::fs::create_dir(&saved)?;
    b.save_item(files_id, saved.clone()).await?;
    assert!(saved.join("工程/嵌套/空目录").is_dir());
    b.submit_content(ClipboardContent::Files(received.clone()))
        .await?;
    until("文件反向传输", || {
        a.history().first().is_some_and(|i| {
            i.kind == ContentKind::Files && i.status == "complete" && i.id == b.history()[0].id
        })
    })
    .await?;

    // Preserve clipboard file roots even when more than 10 new intents evict their history row.
    for n in 0..12 {
        wire.register(&intent(ContentKind::Text, &format!("pending {n}")))
            .await?;
    }
    until("历史淘汰", || {
        b.history().len() == 10 && !b.history().iter().any(|i| i.id == files_id)
    })
    .await?;
    assert!(received.iter().all(|p| p.exists()));
    assert!(matches!(clip_b.content(), Some(ClipboardContent::Files(_))));

    // A previously registered slow file finishes after a newer text: only history may change.
    let slow = wire
        .register(&intent(ContentKind::Files, "slow archive"))
        .await?;
    let tar = temp.path().join("slow.tar");
    let (length, hash) = easycopy_client_core::archive::pack(&roots, &tar)?;
    let new_id = a
        .submit_content(ClipboardContent::Text("新的复制意图优先".into()))
        .await?;
    until("后复制的文本", || {
        clip_b.content() == Some(ClipboardContent::Text("新的复制意图优先".into()))
    })
    .await?;
    wire.upload(slow.id, &tar, length, &hash).await?;
    until("旧文件只补历史", || {
        b.history()
            .iter()
            .any(|i| i.id == slow.id && i.status == "complete")
    })
    .await?;
    assert_eq!(
        clip_b.content(),
        Some(ClipboardContent::Text("新的复制意图优先".into()))
    );
    assert_eq!(b.history()[0].id, new_id);

    // History copy creates a fresh event identity and fresh server order.
    let original = wire.query(new_id).await?;
    b.copy_item(new_id).await?;
    until("历史复制创建新意图", || {
        b.history()[0].id != new_id
            && a.history()[0].id == b.history()[0].id
            && a.history()[0].status == "complete"
    })
    .await?;
    let recopied = wire.query(b.history()[0].id).await?;
    assert!(recopied.sequence > original.sequence);

    // Snapshot includes a not-yet-complete record; its late completion must remain historical.
    let pending = wire
        .register(&intent(ContentKind::Text, "pre-snapshot"))
        .await?;
    let clip_c = Arc::new(MemoryClipboard::default());
    let c = ClientCore::open_with_clipboard(temp.path().join("c"), clip_c.clone())?;
    c.start().await;
    let offline_id = c
        .submit_content(ClipboardContent::Text("离线当前内容".into()))
        .await?;
    until("离线保存", || {
        c.history()
            .iter()
            .any(|i| i.id == offline_id && i.status == "local")
    })
    .await?;
    c.save_settings(input(&pairing, port)).await?;
    until("离线重连快照", || c.is_connected()).await?;
    let pending_path = temp.path().join("pending.txt");
    std::fs::write(&pending_path, "旧任务晚到")?;
    let (length, hash) = easycopy_client_core::archive::hash_file(&pending_path)?;
    wire.upload(pending.id, &pending_path, length, &hash)
        .await?;
    until("快照旧任务完成", || {
        c.history()
            .iter()
            .any(|i| i.id == pending.id && i.status == "complete")
    })
    .await?;
    assert_eq!(
        clip_c.content(),
        Some(ClipboardContent::Text("离线当前内容".into()))
    );
    assert!(wire.query(offline_id).await.is_err(), "离线复制不得重放");

    let (one, two) = tokio::join!(
        a.submit_content(ClipboardContent::Text("并发 A".into())),
        b.submit_content(ClipboardContent::Text("并发 B".into()))
    );
    let one = one?;
    let two = two?;
    until("并发双意图完成", || {
        relay
            .item(one)
            .ok()
            .flatten()
            .is_some_and(|i| i.status == ItemStatus::Complete)
            && relay
                .item(two)
                .ok()
                .flatten()
                .is_some_and(|i| i.status == ItemStatus::Complete)
    })
    .await?;
    let last = if relay.item(one)?.unwrap().sequence > relay.item(two)?.unwrap().sequence {
        "并发 A"
    } else {
        "并发 B"
    };
    until("跨设备并发按服务器登记顺序收敛", || {
        clip_a.content() == Some(ClipboardContent::Text(last.into()))
            && clip_b.content() == Some(ClipboardContent::Text(last.into()))
    })
    .await?;

    let events = observations.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| e.direction == "upload" && e.phase == "transferring" && e.received_bytes > 0));
    assert!(events
        .iter()
        .any(|e| e.direction == "download" && e.phase == "transferring" && e.received_bytes > 0));
    assert!(events
        .iter()
        .filter(|e| e.phase == "complete")
        .all(|e| e.total_bytes == Some(e.received_bytes)));
    drop(events);
    a.shutdown();
    b.shutdown();
    c.shutdown();
    for watcher in watchers {
        watcher.abort();
    }
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_secret_untrusted_ca_and_wrong_hostname_fail() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let task = tokio::spawn(serve_relay(relay.clone(), address));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let trusted = Transport::new("127.0.0.1", address.port(), pairing.clone()).await?;
    trusted.info().await?;
    let mut wrong = pairing.clone();
    wrong.secret = "0".repeat(64);
    let wrong_secret = Transport::new("127.0.0.1", address.port(), wrong).await?;
    assert!(wrong_secret.info().await.is_err());
    assert!(wrong_secret.events().await.is_err());
    let other = Relay::open(temp.path().join("other"))?;
    let mut wrong = pairing.clone();
    wrong.ca_pem = other.pairing().ca_pem;
    let wrong_ca = Transport::new("127.0.0.1", address.port(), wrong).await?;
    assert!(wrong_ca.info().await.is_err());
    assert!(wrong_ca.events().await.is_err());
    let mut wrong = pairing.clone();
    wrong.server_id = Uuid::new_v4();
    wrong.logical_name = format!("relay.{}.invalid", wrong.server_id);
    let wrong_name = Transport::new("127.0.0.1", address.port(), wrong).await?;
    assert!(wrong_name.info().await.is_err());
    assert!(wrong_name.events().await.is_err());
    // The normal public trust store must not silently accept this private CA.
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve(
            &pairing.logical_name,
            SocketAddr::from(([127, 0, 0, 1], address.port())),
        )
        .build()?;
    assert!(client
        .get(format!(
            "https://{}:{}/v1/info",
            pairing.logical_name,
            address.port()
        ))
        .send()
        .await
        .is_err());
    task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_waits_for_verified_response_longer_than_45_seconds() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &relay.identity.certificate_path,
        &relay.identity.private_key_path,
    )
    .await?;
    let app = easycopy_relay::router(relay).layer(axum::middleware::from_fn(
        |request: axum::extract::Request, next: axum::middleware::Next| async move {
            let upload = request.method() == axum::http::Method::PUT;
            let response = next.run(request).await;
            // A large real upload receives no response headers until complete. Model
            // that wait without using a large file or consuming memory for 46 seconds.
            if upload {
                tokio::time::sleep(Duration::from_secs(46)).await;
            }
            response
        },
    ));
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(address, config)
            .serve(app.into_make_service())
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let wire = Transport::new("127.0.0.1", address.port(), pairing).await?;
    let item = wire
        .register(&intent(ContentKind::Text, "delayed verified response"))
        .await?;
    let path = temp.path().join("payload");
    std::fs::write(&path, "streamed payload")?;
    let (size, hash) = easycopy_client_core::archive::hash_file(&path)?;
    let before = Instant::now();
    let complete = tokio::time::timeout(
        Duration::from_secs(55),
        wire.upload(item.id, &path, size, &hash),
    )
    .await??;
    assert!(before.elapsed() >= Duration::from_secs(46));
    assert_eq!(complete.status, ItemStatus::Complete);
    server.abort();
    Ok(())
}
