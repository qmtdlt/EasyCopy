//! Opt-in, synthetic benchmark. Never opens the native clipboard or user profiles.
//! Run with `cargo run -p easycopy-integration-tests --example transfer_bench`.
use anyhow::{bail, Result};
use easycopy_client_core::{archive, transport::Transport};
use easycopy_protocol::{ContentKind, Pairing, RegisterItem};
use easycopy_relay::{serve_relay, Relay};
use std::{
    io::Write,
    net::TcpListener,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mib: usize = std::env::var("EASYCOPY_BENCH_MIB")
        .unwrap_or_else(|_| "333".into())
        .parse()?;
    anyhow::ensure!(
        (1..=1024).contains(&mib),
        "benchmark size must be 1..=1024 MiB"
    );
    archive::ensure_space(temp.path(), (mib as u64) * 1024 * 1024 * 6)?;
    let source = temp.path().join("synthetic.bin");
    let mut file = std::fs::File::create(&source)?;
    // Deterministic nonzero data, no sparse files or user input; generation is not timed.
    let mut block = vec![0u8; 1024 * 1024];
    let mut rng = 0x123456789abcdef_u64;
    for byte in &mut block {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        *byte = rng as u8;
    }
    for _ in 0..mib {
        file.write_all(&block)?;
    }
    file.sync_all()?;
    drop(file);
    let source_hash = archive::hash_file(&source)?;

    // Remote mode is only for a disposable relay with a fresh identity/volume.
    // Never pass a daily-use pairing key: registering would notify its clients.
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (wire, server, scope) = if args.is_empty() {
        let relay = Relay::open(temp.path().join("relay"))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        drop(listener);
        let server = tokio::spawn(serve_relay(relay.clone(), address));
        (
            Transport::new("127.0.0.1", address.port(), relay.pairing()).await?,
            Some(server),
            "localhost TLS",
        )
    } else {
        anyhow::ensure!(
            args.len() == 4 && args[0] == "--isolated-relay",
            "usage: [--isolated-relay HOST PORT PRIVATE_PAIRING_FILE]"
        );
        let pairing = Pairing::decode(std::fs::read_to_string(&args[3])?.trim())?;
        (
            Transport::new(&args[1], args[2].parse()?, pairing).await?,
            None,
            "remote isolated relay TLS",
        )
    };
    for attempt in 0..100 {
        if wire.info().await.is_ok() {
            break;
        }
        if attempt == 99 {
            bail!("isolated relay startup timed out");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let registration = RegisterItem {
        id: Uuid::new_v4(),
        device_id: Uuid::new_v4(),
        device_name: "isolated benchmark".into(),
        kind: ContentKind::Files,
        created_at: 0,
        preview: "synthetic benchmark".into(),
    };
    wire.register(&registration).await?;
    let payload = temp.path().join("payload.tar");
    let start = Instant::now();
    let (bytes, sha) = archive::pack(&[source], &payload)?;
    let pack_s = start.elapsed().as_secs_f64();
    eprintln!("pack: {pack_s:.3}s");
    let start = Instant::now();
    let snapshot = archive::unpack(&payload, &temp.path().join("snapshot"))?;
    let snapshot_s = start.elapsed().as_secs_f64();
    eprintln!("snapshot: {snapshot_s:.3}s");
    let start = Instant::now();
    let legacy = std::env::var_os("EASYCOPY_BENCH_LEGACY_PREPARE").is_some();
    if legacy {
        assert_eq!(archive::hash_file(&payload)?, (bytes, sha.clone()));
    }
    let redundant_hash_s = start.elapsed().as_secs_f64();
    eprintln!("extra hash: {redundant_hash_s:.3}s");
    let start = Instant::now();
    let item = wire.upload(registration.id, &payload, bytes, &sha).await?;
    let upload_s = start.elapsed().as_secs_f64();
    eprintln!("upload including relay verification: {upload_s:.3}s");
    let download = temp.path().join("download.tar");
    let events = AtomicU64::new(0);
    let last = AtomicU64::new(0);
    let start = Instant::now();
    wire.download(&item, &download, |n| {
        events.fetch_add(1, Ordering::Relaxed);
        last.store(n, Ordering::Relaxed);
    })
    .await?;
    let download_s = start.elapsed().as_secs_f64();
    eprintln!("download including verification: {download_s:.3}s");
    assert_eq!(last.load(Ordering::Relaxed), bytes);
    let start = Instant::now();
    let restored = archive::unpack(&download, &temp.path().join("received"))?;
    let restore_s = start.elapsed().as_secs_f64();
    // Extra independent verification is excluded from stage measurements.
    assert_eq!(archive::hash_file(&snapshot[0])?, source_hash);
    assert_eq!(archive::hash_file(&restored[0])?, source_hash);
    println!(
        "{}",
        serde_json::json!({
            "scope": scope, "debug": cfg!(debug_assertions),
            "source_mib": mib, "payload_bytes": bytes, "legacy_prepare": legacy,
            "pack_s": pack_s, "snapshot_s": snapshot_s, "redundant_hash_s": redundant_hash_s,
            "upload_verified_s": upload_s, "download_verified_s": download_s, "restore_s": restore_s,
            "total_s": pack_s + snapshot_s + redundant_hash_s + upload_s + download_s + restore_s,
            "download_progress_events": events.load(Ordering::Relaxed), "verified": true
        })
    );
    if let Some(server) = server {
        server.abort();
    }
    Ok(())
}
