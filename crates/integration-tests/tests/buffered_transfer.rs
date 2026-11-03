use anyhow::{bail, Result};
use easycopy_client_core::{archive, transport::Transport};
use easycopy_protocol::{ContentKind, RegisterItem};
use easycopy_relay::{serve_relay, Relay};
use std::{net::TcpListener, sync::Mutex, time::Duration};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_download_flushes_tails_and_rejects_invalid_payloads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let relay = Relay::open(temp.path().join("relay"))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let server = tokio::spawn(serve_relay(relay.clone(), address));
    let wire = Transport::new("127.0.0.1", address.port(), relay.pairing()).await?;
    for attempt in 0..100 {
        if wire.info().await.is_ok() {
            break;
        }
        if attempt == 99 {
            bail!("isolated relay startup timed out");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for size in [0, 7, 256 * 1024, 2 * 256 * 1024 + 123] {
        let source = temp.path().join(format!("source-{size}"));
        let data: Vec<_> = (0..size).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &data)?;
        let (bytes, hash) = archive::hash_file(&source)?;
        let registration = RegisterItem {
            id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            device_name: "test".into(),
            kind: ContentKind::Files,
            created_at: 0,
            preview: "test".into(),
        };
        wire.register(&registration).await?;
        let item = wire.upload(registration.id, &source, bytes, &hash).await?;
        let destination = temp.path().join(format!("download-{size}"));
        let progress = Mutex::new(Vec::new());
        wire.download(&item, &destination, |n| progress.lock().unwrap().push(n))
            .await?;
        assert_eq!(std::fs::read(&destination)?, data);
        let progress = progress.into_inner().unwrap();
        assert_eq!(progress.last(), Some(&bytes));
        assert_eq!(progress.iter().filter(|&&n| n == bytes).count(), 1);
        assert!(progress.windows(2).all(|pair| pair[0] < pair[1]));
        // Existing cache files must never be overwritten.
        assert!(wire.download(&item, &destination, |_| {}).await.is_err());
        assert_eq!(std::fs::read(&destination)?, data);
        let mut invalid = item.clone();
        invalid.sha256 = Some("0".repeat(64));
        let invalid_progress = Mutex::new(Vec::new());
        assert!(wire
            .download(
                &invalid,
                &temp.path().join(format!("bad-hash-{size}")),
                |n| invalid_progress.lock().unwrap().push(n)
            )
            .await
            .is_err());
        assert!(!invalid_progress.lock().unwrap().contains(&bytes));
        invalid = item;
        invalid.bytes = Some(bytes + 1);
        assert!(wire
            .download(
                &invalid,
                &temp.path().join(format!("bad-size-{size}")),
                |_| {}
            )
            .await
            .is_err());
    }
    server.abort();
    Ok(())
}
