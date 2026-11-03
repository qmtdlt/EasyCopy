use easycopy_protocol::{
    ContentKind, Item, ItemStatus, RegisterItem, ServerEvent, DISK_RESERVE_BYTES,
    PAYLOAD_HASH_HEADER, PAYLOAD_SIZE_HEADER,
};
use easycopy_relay::{router, Relay};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio_tungstenite::{
    client_async_tls_with_config, tungstenite::client::IntoClientRequest, Connector,
    MaybeTlsStream, WebSocketStream,
};
use uuid::Uuid;

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Fixture {
    _dir: TempDir,
    relay: Arc<Relay>,
    client: Client,
    base: String,
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(reserve: u64) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open_with_reserve(dir.path(), reserve).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            &relay.identity.certificate_path,
            &relay.identity.private_key_path,
        )
        .await
        .unwrap();
        let app = router(relay.clone());
        let task = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .serve(app.into_make_service())
                .await
                .unwrap();
        });
        let p = relay.pairing();
        let client = Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(p.ca_pem.as_bytes()).unwrap())
            .resolve(&p.logical_name, addr)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let base = format!("https://{}:{}", p.logical_name, addr.port());
        Self {
            _dir: dir,
            relay,
            client,
            base,
            addr,
            task,
        }
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.relay.pairing().secret)
    }
    async fn register(&self, req: &RegisterItem) -> Item {
        self.request(reqwest::Method::POST, "/v1/items")
            .json(req)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn upload(&self, id: Uuid, payload: &[u8]) -> reqwest::Response {
        self.request(reqwest::Method::PUT, &format!("/v1/items/{id}/payload"))
            .header(PAYLOAD_SIZE_HEADER, payload.len())
            .header(PAYLOAD_HASH_HEADER, hex::encode(Sha256::digest(payload)))
            .body(payload.to_vec())
            .send()
            .await
            .unwrap()
    }
    async fn socket(&self) -> Ws {
        let p = self.relay.pairing();
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut p.ca_pem.as_bytes()) {
            roots.add(cert.unwrap()).unwrap();
        }
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut req = format!("wss://{}:{}/v1/events", p.logical_name, self.addr.port())
            .into_client_request()
            .unwrap();
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", p.secret).parse().unwrap(),
        );
        let tcp = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        client_async_tls_with_config(req, tcp, None, Some(Connector::Rustls(Arc::new(config))))
            .await
            .unwrap()
            .0
    }
}
fn intent(label: &str) -> RegisterItem {
    RegisterItem {
        id: Uuid::new_v4(),
        device_id: Uuid::new_v4(),
        device_name: "测试设备".into(),
        kind: ContentKind::Text,
        created_at: chrono::Utc::now().timestamp_millis(),
        preview: label.into(),
    }
}
async fn next_event(socket: &mut Ws) -> ServerEvent {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if msg.is_text() {
            return serde_json::from_str(msg.to_text().unwrap()).unwrap();
        }
        if msg.is_ping() {
            socket
                .send(tokio_tungstenite::tungstenite::Message::Pong(
                    msg.into_data(),
                ))
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn trusted_tls_authentication_and_identity_validation() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    let response = f
        .request(reqwest::Method::GET, "/v1/info")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = f
        .client
        .get(format!("{}/v1/info", f.base))
        .bearer_auth("wrong-key")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let p = f.relay.pairing();
    let untrusted = Client::builder()
        .no_proxy()
        .resolve(&p.logical_name, f.addr)
        .build()
        .unwrap();
    assert!(untrusted
        .get(format!("{}/v1/info", f.base))
        .send()
        .await
        .is_err());
    assert!(f
        .client
        .get(format!("https://127.0.0.1:{}/v1/info", f.addr.port()))
        .send()
        .await
        .is_err());
}

#[tokio::test]
async fn registration_is_idempotent_and_order_does_not_follow_upload_completion() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    let a = intent("slow file");
    let b = intent("new text");
    let first = f.register(&a).await;
    let second = f.register(&b).await;
    assert!(first.sequence < second.sequence);
    assert_eq!(f.register(&a).await, first);
    let mut conflict = a.clone();
    conflict.preview = "different".into();
    assert_eq!(
        f.request(reqwest::Method::POST, "/v1/items")
            .json(&conflict)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.upload(second.id, b"new text").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        f.upload(first.id, b"slow file").await.status(),
        StatusCode::OK
    );
    let mut socket = f.socket().await;
    let ServerEvent::Snapshot { items, high_water } = next_event(&mut socket).await else {
        panic!()
    };
    assert_eq!(high_water, second.sequence);
    assert_eq!(
        items.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![second.id, first.id]
    );
    assert!(items.iter().all(|i| i.status == ItemStatus::Complete));
    // Response loss recovery uses GET and a same-content PUT returns the same identity/sequence.
    let again: Item = f.upload(first.id, b"slow file").await.json().await.unwrap();
    assert_eq!(again.sequence, first.sequence);
    assert_eq!(
        f.upload(first.id, b"different").await.status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn streamed_payload_progress_hash_failure_retry_and_download() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    let mut socket = f.socket().await;
    assert!(matches!(
        next_event(&mut socket).await,
        ServerEvent::Snapshot { high_water: 0, .. }
    ));
    let record = f.register(&intent("payload")).await;
    assert!(
        matches!(next_event(&mut socket).await, ServerEvent::Registered { item } if item.id == record.id)
    );
    let bad = f
        .request(
            reqwest::Method::PUT,
            &format!("/v1/items/{}/payload", record.id),
        )
        .header(PAYLOAD_SIZE_HEADER, 3)
        .header(PAYLOAD_HASH_HEADER, "0".repeat(64))
        .body("abc")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        f.relay.item(record.id).unwrap().unwrap().status,
        ItemStatus::Failed
    );
    let payload = vec![0x7bu8; 3 * 1024 * 1024 + 17];
    let uploaded: Item = f
        .upload(record.id, &payload)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(uploaded.status, ItemStatus::Complete);
    assert_eq!(
        uploaded.sha256.as_deref(),
        Some(hex::encode(Sha256::digest(&payload)).as_str())
    );
    let response = f
        .request(
            reqwest::Method::GET,
            &format!("/v1/items/{}/payload", record.id),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap().as_ref(), payload);
    let mut final_bytes = 0;
    let mut failed_seen = false;
    loop {
        match next_event(&mut socket).await {
            ServerEvent::Progress {
                received_bytes,
                total_bytes,
                ..
            } => {
                assert!(received_bytes <= total_bytes);
                final_bytes = received_bytes;
            }
            ServerEvent::Failed { .. } => failed_seen = true,
            ServerEvent::Completed { item } => {
                assert_eq!(item.id, record.id);
                break;
            }
            _ => {}
        }
    }
    assert!(failed_seen);
    assert_eq!(final_bytes, payload.len() as u64);
    assert!(!f
        .relay
        .data_dir()
        .join("payloads")
        .join(format!("{}.part", record.id))
        .exists());
}

#[tokio::test]
async fn cancellation_is_a_tombstone_and_disk_reserve_rejects_before_upload() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    let request = intent("cancel");
    let record = f.register(&request).await;
    let cancelled: Item = f
        .request(reqwest::Method::DELETE, &format!("/v1/items/{}", record.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cancelled.status, ItemStatus::Cancelled);
    assert_eq!(f.register(&request).await.status, ItemStatus::Cancelled);
    assert_eq!(
        f.upload(record.id, b"hello").await.status(),
        StatusCode::GONE
    );
    let f = Fixture::new(u64::MAX).await;
    let record = f.register(&intent("no space")).await;
    assert_eq!(
        f.upload(record.id, b"hello").await.status(),
        StatusCode::INSUFFICIENT_STORAGE
    );
    assert_eq!(
        f.relay.item(record.id).unwrap().unwrap().status,
        ItemStatus::Registered
    );
}

#[tokio::test]
async fn snapshot_has_ten_records_and_global_highwater_including_unfinished_intents() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    for n in 0..12 {
        f.register(&intent(&format!("{n}"))).await;
    }
    let mut socket = f.socket().await;
    let ServerEvent::Snapshot { items, high_water } = next_event(&mut socket).await else {
        panic!()
    };
    assert_eq!(items.len(), 10);
    assert_eq!(high_water, 12);
    assert_eq!(items.first().unwrap().sequence, 12);
    assert_eq!(items.last().unwrap().sequence, 3);
    let next = f.register(&intent("live after snapshot")).await;
    assert_eq!(next.sequence, high_water + 1);
    assert!(
        matches!(next_event(&mut socket).await, ServerEvent::Registered { item } if item.id == next.id)
    );
}

#[test]
fn identity_persists_without_exposing_private_key_in_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let identity = easycopy_relay::identity::Identity::load_or_create(dir.path()).unwrap();
    let again = easycopy_relay::identity::Identity::load_or_create(dir.path()).unwrap();
    assert_eq!(
        identity.pairing.encode().unwrap(),
        again.pairing.encode().unwrap()
    );
    assert!(!identity.pairing.ca_pem.contains("PRIVATE KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(identity.private_key_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn truncated_stream_fails_and_retries_same_identity_from_zero() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    let record = f.register(&intent("interrupted")).await;
    let payload = b"abcdefghij";
    let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
        bytes::Bytes::from_static(b"abc"),
    )]);
    let response = f
        .request(
            reqwest::Method::PUT,
            &format!("/v1/items/{}/payload", record.id),
        )
        .header(PAYLOAD_SIZE_HEADER, payload.len())
        .header(PAYLOAD_HASH_HEADER, hex::encode(Sha256::digest(payload)))
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        f.relay.item(record.id).unwrap().unwrap().status,
        ItemStatus::Failed
    );
    let recovered: Item = f
        .upload(record.id, payload)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered.id, record.id);
    assert_eq!(recovered.sequence, record.sequence);
    assert_eq!(recovered.status, ItemStatus::Complete);
}

#[tokio::test]
async fn concurrent_upload_rejected_and_cancel_interrupts_a_stalled_stream() {
    let f = Fixture::new(DISK_RESERVE_BYTES).await;
    let record = f.register(&intent("stalled")).await;
    let stream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
        bytes::Bytes::from_static(b"a"),
    )])
    .chain(futures_util::stream::pending());
    let request = f
        .request(
            reqwest::Method::PUT,
            &format!("/v1/items/{}/payload", record.id),
        )
        .header(PAYLOAD_SIZE_HEADER, 10)
        .header(
            PAYLOAD_HASH_HEADER,
            hex::encode(Sha256::digest(b"abcdefghij")),
        )
        .body(reqwest::Body::wrap_stream(stream));
    let transfer = tokio::spawn(async move { request.send().await });
    for _ in 0..100 {
        if f.relay.item(record.id).unwrap().unwrap().status == ItemStatus::Uploading {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        f.relay.item(record.id).unwrap().unwrap().status,
        ItemStatus::Uploading
    );
    assert_eq!(
        f.upload(record.id, b"abcdefghij").await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.request(reqwest::Method::DELETE, &format!("/v1/items/{}", record.id))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let response = tokio::time::timeout(Duration::from_secs(2), transfer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    assert_eq!(
        f.relay.item(record.id).unwrap().unwrap().status,
        ItemStatus::Cancelled
    );
    assert!(!f
        .relay
        .data_dir()
        .join("payloads")
        .join(format!("{}.part", record.id))
        .exists());
}
