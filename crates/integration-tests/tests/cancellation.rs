use anyhow::{bail, Result};
use axum::{extract::Request, middleware::Next};
use easycopy_client_core::{
    transport::Transport, ClientCore, ClipboardContent, MemoryClipboard, SettingsInput,
};
use easycopy_protocol::{ContentKind, ItemStatus, RegisterItem};
use easycopy_relay::Relay;
use std::{
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;
use uuid::Uuid;

async fn until(mut condition: impl FnMut() -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_copy_cancels_stalled_upload_and_survives_reconnect() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let held = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first = Arc::new(AtomicBool::new(true));
    let app = easycopy_relay::router(relay.clone()).layer(axum::middleware::from_fn({
        let held = held.clone();
        let release = release.clone();
        let first = first.clone();
        move |request: Request, next: Next| {
            let held = held.clone();
            let release = release.clone();
            let first = first.clone();
            async move {
                if request.method() == axum::http::Method::PUT
                    && first.swap(false, Ordering::SeqCst)
                {
                    held.notify_one();
                    release.notified().await;
                }
                next.run(request).await
            }
        }
    }));
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &relay.identity.certificate_path,
        &relay.identity.private_key_path,
    )
    .await?;
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(address, config)
            .serve(app.into_make_service())
            .await
    });
    let wire = Transport::new("127.0.0.1", address.port(), pairing.clone()).await?;
    for _ in 0..100 {
        if wire.info().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let clip_a = Arc::new(MemoryClipboard::default());
    let clip_b = Arc::new(MemoryClipboard::default());
    let a = ClientCore::open_with_clipboard(temp.path().join("a"), clip_a)?;
    let b = ClientCore::open_with_clipboard(temp.path().join("b"), clip_b.clone())?;
    let settings = || SettingsInput {
        address: "127.0.0.1".into(),
        port: address.port(),
        pairing_key: pairing.encode().unwrap(),
    };
    a.save_settings(settings()).await?;
    b.save_settings(settings()).await?;
    a.start().await;
    b.start().await;
    until(|| a.is_connected() && b.is_connected()).await?;
    let old = a
        .submit_content(ClipboardContent::Text("旧内容".into()))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), held.notified()).await?;
    let new = a
        .submit_content(ClipboardContent::Text("新内容".into()))
        .await?;
    until(|| {
        relay
            .item(old)
            .ok()
            .flatten()
            .is_some_and(|i| i.status == ItemStatus::Cancelled)
    })
    .await?;
    until(|| clip_b.content() == Some(ClipboardContent::Text("新内容".into()))).await?;
    until(|| {
        a.history()
            .iter()
            .any(|e| e.id == old && e.status == "cancelled")
            && b.history()
                .iter()
                .any(|e| e.id == old && e.status == "cancelled")
    })
    .await?;
    release.notify_waiters();
    first.store(true, Ordering::SeqCst);
    let manual = a
        .submit_content(ClipboardContent::Text("手动取消".into()))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), held.notified()).await?;
    a.cancel_transfer(manual)?;
    until(|| {
        relay
            .item(manual)
            .ok()
            .flatten()
            .is_some_and(|i| i.status == ItemStatus::Cancelled)
    })
    .await?;
    release.notify_waiters();
    a.save_settings(settings()).await?;
    until(|| a.is_connected()).await?;
    assert_eq!(relay.item(old)?.unwrap().status, ItemStatus::Cancelled);
    assert_eq!(relay.item(new)?.unwrap().status, ItemStatus::Complete);
    a.shutdown();
    b.shutdown();
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_copy_proceeds_when_old_registration_response_is_stalled() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let held = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first = Arc::new(AtomicBool::new(true));
    let app = easycopy_relay::router(relay.clone()).layer(axum::middleware::from_fn({
        let held = held.clone();
        let release = release.clone();
        let first = first.clone();
        move |request: Request, next: Next| {
            let held = held.clone();
            let release = release.clone();
            let first = first.clone();
            async move {
                let hold = request.method() == axum::http::Method::POST
                    && first.swap(false, Ordering::SeqCst);
                let response = next.run(request).await;
                if hold {
                    held.notify_one();
                    release.notified().await;
                }
                response
            }
        }
    }));
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &relay.identity.certificate_path,
        &relay.identity.private_key_path,
    )
    .await?;
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(address, config)
            .serve(app.into_make_service())
            .await
    });
    let wire = Transport::new("127.0.0.1", address.port(), pairing.clone()).await?;
    for _ in 0..100 {
        if wire.info().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let core = ClientCore::open_with_clipboard(
        temp.path().join("client"),
        Arc::new(MemoryClipboard::default()),
    )?;
    core.save_settings(SettingsInput {
        address: "127.0.0.1".into(),
        port: address.port(),
        pairing_key: pairing.encode()?,
    })
    .await?;
    core.start().await;
    until(|| core.is_connected()).await?;
    let old = core
        .submit_content(ClipboardContent::Text("旧内容".into()))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), held.notified()).await?;
    let new = core
        .submit_content(ClipboardContent::Text("新内容".into()))
        .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !relay
            .item(new)
            .ok()
            .flatten()
            .is_some_and(|item| item.status == ItemStatus::Complete)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    until(|| {
        relay
            .item(old)
            .ok()
            .flatten()
            .is_some_and(|item| item.status == ItemStatus::Cancelled)
    })
    .await?;
    assert!(relay.item(old)?.unwrap().sequence < relay.item(new)?.unwrap().sequence);
    release.notify_waiters();
    core.shutdown();
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_upload_is_reported_when_cancellation_is_too_late() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let held = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = easycopy_relay::router(relay.clone()).layer(axum::middleware::from_fn({
        let held = held.clone();
        let release = release.clone();
        move |request: Request, next: Next| {
            let held = held.clone();
            let release = release.clone();
            async move {
                let upload = request.method() == axum::http::Method::PUT;
                let response = next.run(request).await;
                if upload {
                    held.notify_one();
                    release.notified().await;
                }
                response
            }
        }
    }));
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &relay.identity.certificate_path,
        &relay.identity.private_key_path,
    )
    .await?;
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(address, config)
            .serve(app.into_make_service())
            .await
    });
    let wire = Transport::new("127.0.0.1", address.port(), pairing.clone()).await?;
    for _ in 0..100 {
        if wire.info().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let core = ClientCore::open_with_clipboard(
        temp.path().join("client"),
        Arc::new(MemoryClipboard::default()),
    )?;
    core.save_settings(SettingsInput {
        address: "127.0.0.1".into(),
        port: address.port(),
        pairing_key: pairing.encode()?,
    })
    .await?;
    core.start().await;
    until(|| core.is_connected()).await?;
    let id = core
        .submit_content(ClipboardContent::Text("已完成".into()))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), held.notified()).await?;
    assert_eq!(relay.item(id)?.unwrap().status, ItemStatus::Complete);
    core.cancel_transfer(id)?;
    until(|| {
        core.history()
            .iter()
            .any(|e| e.id == id && e.status == "complete" && e.detail.contains("取消过晚"))
    })
    .await?;
    release.notify_one();
    core.shutdown();
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_download_is_local_and_retry_does_not_replace_clipboard() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let pairing = relay.pairing();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let held = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let gate = Arc::new(AtomicBool::new(true));
    let app = easycopy_relay::router(relay.clone()).layer(axum::middleware::from_fn({
        let held = held.clone();
        let release = release.clone();
        let gate = gate.clone();
        move |request: Request, next: Next| {
            let held = held.clone();
            let release = release.clone();
            let gate = gate.clone();
            async move {
                if request.method() == axum::http::Method::GET
                    && request.uri().path().ends_with("/payload")
                    && gate.swap(false, Ordering::SeqCst)
                {
                    held.notify_one();
                    release.notified().await;
                }
                next.run(request).await
            }
        }
    }));
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &relay.identity.certificate_path,
        &relay.identity.private_key_path,
    )
    .await?;
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(address, config)
            .serve(app.into_make_service())
            .await
    });
    let wire = Transport::new("127.0.0.1", address.port(), pairing.clone()).await?;
    for _ in 0..100 {
        if wire.info().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let clip = Arc::new(MemoryClipboard::default());
    clip.set(ClipboardContent::Text("当前内容".into()));
    let core = ClientCore::open_with_clipboard(temp.path().join("client"), clip.clone())?;
    core.save_settings(SettingsInput {
        address: "127.0.0.1".into(),
        port: address.port(),
        pairing_key: pairing.encode()?,
    })
    .await?;
    core.start().await;
    until(|| core.is_connected()).await?;
    let request = RegisterItem {
        id: Uuid::new_v4(),
        device_id: Uuid::new_v4(),
        device_name: "peer".into(),
        kind: ContentKind::Text,
        created_at: 0,
        preview: "文件".into(),
    };
    wire.register(&request).await?;
    let path = temp.path().join("source");
    std::fs::write(&path, "收到的内容")?;
    let (size, hash) = easycopy_client_core::archive::hash_file(&path)?;
    wire.upload(request.id, &path, size, &hash).await?;
    tokio::time::timeout(Duration::from_secs(10), held.notified()).await?;
    core.cancel_transfer(request.id)?;
    until(|| {
        core.history()
            .iter()
            .any(|e| e.id == request.id && e.can_retry_download)
    })
    .await?;
    assert_eq!(
        relay.item(request.id)?.unwrap().status,
        ItemStatus::Complete
    );
    assert!(!temp
        .path()
        .join(format!("client/cache/{}/payload.downloading", request.id))
        .exists());
    release.notify_one();
    core.retry_download(request.id)?;
    until(|| {
        core.history()
            .iter()
            .any(|e| e.id == request.id && e.available && e.status == "complete")
    })
    .await?;
    if clip.content() != Some(ClipboardContent::Text("当前内容".into())) {
        bail!("重新下载覆盖了剪贴板");
    }
    core.shutdown();
    server.abort();
    Ok(())
}
