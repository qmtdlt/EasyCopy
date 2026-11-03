use anyhow::{bail, Result};
use easycopy_client_core::{
    transport::Transport, ClientCore, ClipboardContent, MemoryClipboard, SettingsInput,
    FILE_CONFIRM_BYTES,
};
use easycopy_relay::{serve_relay, Relay};
use std::{net::TcpListener, sync::Arc, time::Duration};

async fn until(condition: impl Fn() -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

// Isolated loopback relay and memory clipboards; never touches the user's server/clipboard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_files_require_yes_and_declined_copies_never_reach_relay() -> Result<()> {
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
    let settings = SettingsInput {
        address: "127.0.0.1".into(),
        port: address.port(),
        pairing_key: relay.pairing().encode()?,
    };
    let clip_a = Arc::new(MemoryClipboard::default());
    let clip_b = Arc::new(MemoryClipboard::default());
    let a = ClientCore::open_with_clipboard(temp.path().join("a"), clip_a.clone())?;
    let b = ClientCore::open_with_clipboard(temp.path().join("b"), clip_b.clone())?;
    a.save_settings(settings.clone()).await?;
    b.save_settings(settings.clone()).await?;
    a.start().await;
    b.start().await;
    until(|| a.is_connected() && b.is_connected()).await?;

    let path = temp.path().join("large.bin");
    std::fs::File::create(&path)?.set_len(FILE_CONFIRM_BYTES + 1)?;
    let content = ClipboardContent::Files(vec![path]);
    let rejected = a.submit_content(content.clone()).await?;
    until(|| a.file_upload_prompt().is_some()).await?;
    assert!(wire.query(rejected).await.is_err());
    assert!(a.history().is_empty() && b.history().is_empty());
    a.resolve_file_upload(rejected, false)?;
    a.save_settings(settings.clone()).await?; // reconnect cannot register declined work
    until(|| a.is_connected()).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(a.file_upload_prompt().is_none());
    assert!(wire.query(rejected).await.is_err());
    assert!(b.history().is_empty());
    assert_eq!(clip_a.content(), Some(content.clone()));

    let accepted = a.submit_content(content).await?;
    until(|| a.file_upload_prompt().is_some()).await?;
    assert!(wire.query(accepted).await.is_err());
    a.resolve_file_upload(accepted, true)?;
    until(|| b.history().iter().any(|e| e.id == accepted && e.available)).await?;
    let Some(ClipboardContent::Files(received)) = clip_b.content() else {
        bail!("missing received files")
    };
    assert_eq!(
        std::fs::metadata(&received[0])?.len(),
        FILE_CONFIRM_BYTES + 1
    );
    assert!(b.copy_item(accepted).await.is_err());
    let saved = temp.path().join("saved");
    std::fs::create_dir(&saved)?;
    b.save_item(accepted, saved.clone()).await?;
    assert_eq!(
        std::fs::metadata(saved.join("large.bin"))?.len(),
        FILE_CONFIRM_BYTES + 1
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.history().len(), 1);
    assert_eq!(b.history().len(), 1);
    assert!(b.file_upload_prompt().is_none()); // remote write never asks or sends back
    a.shutdown();
    b.shutdown();
    server.abort();
    Ok(())
}
