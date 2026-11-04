pub mod identity;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket},
        DefaultBodyLimit, Path, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use easycopy_protocol::{
    ApiError, Item, ItemStatus, Pairing, RegisterItem, ServerEvent, ServerInfo, DISK_RESERVE_BYTES,
    HISTORY_LIMIT, PAYLOAD_HASH_HEADER, PAYLOAD_SIZE_HEADER,
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{io::AsyncWriteExt, sync::broadcast};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

pub struct Relay {
    pub identity: identity::Identity,
    data_dir: PathBuf,
    state: Mutex<RelayState>,
    _process_lock: fs::File,
    disk_reserve: u64,
    cleanup_started: AtomicBool,
}
struct CancelSignal {
    cancelled: AtomicBool,
    wake: tokio::sync::Notify,
}
struct ActiveUpload {
    remaining: u64,
    cancelled: Arc<CancelSignal>,
}
struct RelayState {
    db: Connection,
    events: broadcast::Sender<ServerEvent>,
    active: HashMap<Uuid, ActiveUpload>,
    downloads: HashMap<Uuid, usize>,
}

#[derive(Debug)]
struct HttpError(StatusCode, String);
type HttpResult<T> = std::result::Result<T, HttpError>;
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.0, Json(ApiError { error: self.1 })).into_response()
    }
}
impl From<anyhow::Error> for HttpError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(error = %error, "relay operation failed");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "服务器读写失败".into())
    }
}
fn err(code: StatusCode, message: impl Into<String>) -> HttpError {
    HttpError(code, message.into())
}

impl Relay {
    pub fn open(data_dir: impl AsRef<FsPath>) -> Result<Arc<Self>> {
        Self::open_with_reserve(data_dir, DISK_RESERVE_BYTES)
    }
    // A larger reserve allows integration tests to exercise ENOSPC without filling a disk.
    pub fn open_with_reserve(data_dir: impl AsRef<FsPath>, disk_reserve: u64) -> Result<Arc<Self>> {
        anyhow::ensure!(disk_reserve >= DISK_RESERVE_BYTES, "安全余量不得低于 1 GiB");
        let data_dir = data_dir.as_ref().to_path_buf();
        identity::secure_directory(&data_dir)?;
        let identity = identity::Identity::load_or_create(&data_dir)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir.join("relay.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock).context("此数据目录已有中转服务运行")?;
        identity::secure_directory(&data_dir.join("payloads"))?;
        let db = Connection::open(data_dir.join("metadata.sqlite3"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS items (sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, record TEXT NOT NULL);")?;
        {
            let mut statement = db.prepare("SELECT record FROM items")?;
            let records = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for record in records {
                let mut item: Item = serde_json::from_str(&record)?;
                if item.status == ItemStatus::Uploading {
                    item.status = ItemStatus::Failed;
                    item.error = Some("服务重启中断上传，可从头重试".into());
                    put_item(&db, &item)?;
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                data_dir.join("metadata.sqlite3"),
                fs::Permissions::from_mode(0o600),
            )?;
        }
        let (events, _) = broadcast::channel(1024);
        let relay = Arc::new(Self {
            identity,
            data_dir,
            state: Mutex::new(RelayState {
                db,
                events,
                active: HashMap::new(),
                downloads: HashMap::new(),
            }),
            _process_lock: lock,
            disk_reserve,
            cleanup_started: AtomicBool::new(false),
        });
        // No transfer survives a restart. Sweep legacy backlog and orphan files
        // before accepting requests, without deleting sequence/idempotency metadata.
        relay.cleanup_payloads(&relay.state.lock().unwrap())?;
        Ok(relay)
    }
    pub fn pairing(&self) -> Pairing {
        self.identity.pairing.clone()
    }
    pub fn info(&self) -> ServerInfo {
        self.identity.info()
    }
    pub fn data_dir(&self) -> &FsPath {
        &self.data_dir
    }
    pub fn item(&self, id: Uuid) -> Result<Option<Item>> {
        get_item(&self.state.lock().unwrap().db, id)
    }
    fn payload_path(&self, id: Uuid) -> PathBuf {
        self.data_dir.join("payloads").join(format!("{id}.payload"))
    }
    fn part_path(&self, id: Uuid) -> PathBuf {
        self.data_dir.join("payloads").join(format!("{id}.part"))
    }
    fn retained(state: &RelayState, id: Uuid) -> Result<bool> {
        Ok(state.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM (SELECT id FROM items ORDER BY sequence DESC LIMIT ?1) WHERE id=?2)",
            params![HISTORY_LIMIT as u64, id.to_string()],
            |row| row.get(0),
        )?)
    }
    // All admission, publication, lease release and collection use the same lock.
    fn cleanup_payloads(&self, state: &RelayState) -> Result<()> {
        let mut statement = state
            .db
            .prepare("SELECT record FROM items ORDER BY sequence DESC LIMIT ?1")?;
        let records = statement.query_map([HISTORY_LIMIT as u64], |row| row.get::<_, String>(0))?;
        let mut keep = HashSet::new();
        for record in records {
            let item: Item = serde_json::from_str(&record?)?;
            if item.status == ItemStatus::Complete {
                keep.insert(item.id);
            }
        }
        let mut failure = None;
        for entry in fs::read_dir(self.data_dir.join("payloads"))? {
            let path = entry?.path();
            let extension = path.extension().and_then(|s| s.to_str());
            if !matches!(extension, Some("payload" | "part")) {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            let protected = id.is_some_and(|id| {
                state.active.contains_key(&id)
                    || (extension == Some("payload")
                        && (keep.contains(&id) || state.downloads.contains_key(&id)))
            });
            if !protected {
                if let Err(error) = fs::remove_file(&path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(path = %path.display(), %error, "payload cleanup failed; will retry");
                        failure = Some(error);
                    }
                }
            }
        }
        if let Some(error) = failure {
            return Err(error.into());
        }
        Ok(())
    }
    fn cleanup_best_effort(&self, state: &RelayState) {
        if let Err(error) = self.cleanup_payloads(state) {
            tracing::warn!(%error, "payload cleanup incomplete; will retry");
        }
    }
    fn start_cleanup(self: &Arc<Self>, interval: Duration) {
        if self.cleanup_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(relay) = weak.upgrade() else { break };
                let _ = tokio::task::spawn_blocking(move || {
                    relay.cleanup_best_effort(&relay.state.lock().unwrap());
                })
                .await;
            }
        });
    }
    fn snapshot(&self) -> Result<(ServerEvent, broadcast::Receiver<ServerEvent>)> {
        let state = self.state.lock().unwrap();
        let high_water: u64 =
            state
                .db
                .query_row("SELECT COALESCE(MAX(sequence), 0) FROM items", [], |r| {
                    r.get(0)
                })?;
        let mut statement = state
            .db
            .prepare("SELECT record FROM items ORDER BY sequence DESC LIMIT ?1")?;
        let records = statement
            .query_map([HISTORY_LIMIT as u64], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let items = records
            .iter()
            .map(|s| serde_json::from_str(s))
            .collect::<std::result::Result<Vec<Item>, _>>()?;
        let subscription = state.events.subscribe();
        Ok((ServerEvent::Snapshot { items, high_water }, subscription))
    }
    fn register(&self, request: RegisterItem) -> HttpResult<Item> {
        if request.id.is_nil()
            || request.device_id.is_nil()
            || request.device_name.is_empty()
            || request.device_name.len() > 256
            || request.preview.len() > 4096
            || request.device_name.contains('\0')
            || request.preview.contains('\0')
        {
            return Err(err(StatusCode::BAD_REQUEST, "复制记录字段无效或过长"));
        }
        let state = self.state.lock().unwrap();
        if let Some(item) = get_item(&state.db, request.id)? {
            if item.device_id != request.device_id
                || item.kind != request.kind
                || item.device_name != request.device_name
                || item.created_at != request.created_at
                || item.preview != request.preview
            {
                return Err(err(StatusCode::CONFLICT, "记录身份已经用于另一复制意图"));
            }
            return Ok(item);
        }
        let mut item = Item {
            id: request.id,
            device_id: request.device_id,
            device_name: request.device_name,
            kind: request.kind,
            created_at: request.created_at,
            preview: request.preview,
            sequence: 0,
            status: ItemStatus::Registered,
            bytes: None,
            sha256: None,
            error: None,
        };
        // Serialize all registrations; sequence is allocated before payload preparation begins.
        let transaction = state
            .db
            .unchecked_transaction()
            .map_err(anyhow::Error::from)?;
        transaction
            .execute(
                "INSERT INTO items(id,record) VALUES(?1,?2)",
                params![
                    item.id.to_string(),
                    serde_json::to_string(&item).map_err(anyhow::Error::from)?
                ],
            )
            .map_err(anyhow::Error::from)?;
        item.sequence = transaction.last_insert_rowid() as u64;
        put_item(&transaction, &item)?;
        transaction.commit().map_err(anyhow::Error::from)?;
        let _ = state
            .events
            .send(ServerEvent::Registered { item: item.clone() });
        self.cleanup_best_effort(&state);
        Ok(item)
    }
    fn cancel(&self, id: Uuid) -> HttpResult<Item> {
        let state = self.state.lock().unwrap();
        let mut item =
            get_item(&state.db, id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "记录不存在"))?;
        if item.status == ItemStatus::Complete {
            return Err(err(StatusCode::CONFLICT, "已完成的记录不能取消"));
        }
        if let Some(active) = state.active.get(&id) {
            active.cancelled.cancelled.store(true, Ordering::SeqCst);
            active.cancelled.wake.notify_one();
        }
        item.status = ItemStatus::Cancelled;
        item.error = Some("传输已取消".into());
        put_item(&state.db, &item)?;
        let _ = state
            .events
            .send(ServerEvent::Failed { item: item.clone() });
        Ok(item)
    }
}

fn get_item(db: &Connection, id: Uuid) -> Result<Option<Item>> {
    let json: Option<String> = db
        .query_row(
            "SELECT record FROM items WHERE id=?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
fn put_item(db: &Connection, item: &Item) -> Result<()> {
    db.execute(
        "UPDATE items SET record=?2 WHERE id=?1",
        params![item.id.to_string(), serde_json::to_string(item)?],
    )?;
    Ok(())
}

pub fn router(relay: Arc<Relay>) -> Router {
    relay.start_cleanup(Duration::from_secs(60));
    Router::new()
        .route("/v1/info", get(info))
        .route("/v1/items", post(register))
        .route("/v1/items/{id}", get(item).delete(cancel))
        .route(
            "/v1/items/{id}/payload",
            put(upload).get(download).layer(DefaultBodyLimit::disable()),
        )
        .route("/v1/events", get(events))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(relay.clone(), authenticate))
        .with_state(relay)
}
async fn authenticate(
    State(relay): State<Arc<Relay>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let actual = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(
        actual
            .as_bytes()
            .ct_eq(relay.identity.pairing.secret.as_bytes()),
    ) {
        return err(StatusCode::UNAUTHORIZED, "连接密钥错误或缺失").into_response();
    }
    next.run(request).await
}
async fn info(State(relay): State<Arc<Relay>>) -> Json<ServerInfo> {
    Json(relay.info())
}
async fn register(
    State(relay): State<Arc<Relay>>,
    Json(request): Json<RegisterItem>,
) -> HttpResult<Json<Item>> {
    Ok(Json(relay.register(request)?))
}
async fn item(State(relay): State<Arc<Relay>>, Path(id): Path<Uuid>) -> HttpResult<Json<Item>> {
    Ok(Json(
        relay
            .item(id)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "记录不存在"))?,
    ))
}
async fn cancel(State(relay): State<Arc<Relay>>, Path(id): Path<Uuid>) -> HttpResult<Json<Item>> {
    Ok(Json(relay.cancel(id)?))
}

struct UploadGuard {
    relay: Arc<Relay>,
    id: Uuid,
    committed: bool,
}
impl Drop for UploadGuard {
    fn drop(&mut self) {
        let mut state = self.relay.state.lock().unwrap();
        state.active.remove(&self.id);
        if !self.committed {
            if let Ok(Some(mut item)) = get_item(&state.db, self.id) {
                if item.status == ItemStatus::Uploading {
                    item.status = ItemStatus::Failed;
                    item.error = Some("上传中断，可从头重试".into());
                    if put_item(&state.db, &item).is_ok() {
                        let _ = state.events.send(ServerEvent::Failed { item });
                    }
                }
            }
            let _ = fs::remove_file(self.relay.part_path(self.id));
        }
        self.relay.cleanup_best_effort(&state);
    }
}

fn header_number(headers: &HeaderMap, name: &str) -> HttpResult<u64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("缺失或无效的 {name}")))
}

async fn upload(
    State(relay): State<Arc<Relay>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> HttpResult<Json<Item>> {
    let total = header_number(&headers, PAYLOAD_SIZE_HEADER)?;
    let expected = headers
        .get(PAYLOAD_HASH_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, "SHA-256 摘要无效"));
    }
    if let Some(length) = headers.get(header::CONTENT_LENGTH) {
        if length.to_str().ok().and_then(|s| s.parse::<u64>().ok()) != Some(total) {
            return Err(err(StatusCode::BAD_REQUEST, "载荷长度与声明不一致"));
        }
    }
    let cancelled = Arc::new(CancelSignal {
        cancelled: AtomicBool::new(false),
        wake: tokio::sync::Notify::new(),
    });
    {
        let mut state = relay.state.lock().unwrap();
        let mut item = get_item(&state.db, id)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "请先登记复制意图"))?;
        if item.status == ItemStatus::Complete {
            if item.bytes == Some(total) && item.sha256.as_deref() == Some(&expected) {
                return Ok(Json(item));
            }
            return Err(err(StatusCode::CONFLICT, "已完成载荷与本次上传不一致"));
        }
        if !Relay::retained(&state, id)? {
            return Err(err(
                StatusCode::GONE,
                "记录已超出最近 10 条保留范围，请重新复制",
            ));
        }
        if item.status == ItemStatus::Cancelled {
            return Err(err(StatusCode::GONE, "记录已经取消"));
        }
        if state.active.contains_key(&id) {
            return Err(err(StatusCode::CONFLICT, "此记录正在上传"));
        }
        let reserved = state
            .active
            .values()
            .try_fold(0u64, |n, a| n.checked_add(a.remaining))
            .unwrap_or(u64::MAX);
        let required = total
            .checked_add(reserved)
            .and_then(|v| v.checked_add(relay.disk_reserve))
            .unwrap_or(u64::MAX);
        let available = fs2::available_space(&relay.data_dir).map_err(anyhow::Error::from)?;
        if available < required {
            return Err(err(
                StatusCode::INSUFFICIENT_STORAGE,
                "服务器磁盘空间不足，必须保留至少 1 GiB",
            ));
        }
        item.status = ItemStatus::Uploading;
        item.bytes = Some(total);
        item.sha256 = Some(expected.clone());
        item.error = None;
        put_item(&state.db, &item)?;
        state.active.insert(
            id,
            ActiveUpload {
                remaining: total,
                cancelled: cancelled.clone(),
            },
        );
        let _ = state.events.send(ServerEvent::Progress {
            id,
            received_bytes: 0,
            total_bytes: total,
        });
    }
    let mut guard = UploadGuard {
        relay: relay.clone(),
        id,
        committed: false,
    };
    let outcome = receive_payload(&relay, id, total, &expected, body, cancelled).await;
    match outcome {
        Ok(()) => {
            let state = relay.state.lock().unwrap();
            let mut item = get_item(&state.db, id)?.context("record lost")?;
            if item.status == ItemStatus::Cancelled {
                return Err(err(StatusCode::GONE, "传输已经取消"));
            }
            if !Relay::retained(&state, id)? {
                item.status = ItemStatus::Failed;
                item.error = Some("记录已超出最近 10 条保留范围，请重新复制".into());
                put_item(&state.db, &item)?;
                let _ = state.events.send(ServerEvent::Failed { item });
                return Err(err(
                    StatusCode::GONE,
                    "记录已超出最近 10 条保留范围，请重新复制",
                ));
            }
            // Rename and durable metadata transition share the publication lock with cancel/snapshot.
            fs::rename(relay.part_path(id), relay.payload_path(id)).map_err(anyhow::Error::from)?;
            #[cfg(unix)]
            fs::File::open(relay.data_dir.join("payloads"))
                .and_then(|dir| dir.sync_all())
                .map_err(anyhow::Error::from)?;
            item.status = ItemStatus::Complete;
            item.error = None;
            put_item(&state.db, &item)?;
            guard.committed = true;
            let _ = state
                .events
                .send(ServerEvent::Completed { item: item.clone() });
            Ok(Json(item))
        }
        Err(error) => {
            let state = relay.state.lock().unwrap();
            if let Some(mut item) = get_item(&state.db, id)? {
                if item.status != ItemStatus::Cancelled {
                    item.status = ItemStatus::Failed;
                    item.error = Some(error.1.clone());
                    put_item(&state.db, &item)?;
                    let _ = state.events.send(ServerEvent::Failed { item });
                }
            }
            Err(error)
        }
    }
}

async fn receive_payload(
    relay: &Relay,
    id: Uuid,
    total: u64,
    expected: &str,
    body: Body,
    cancelled: Arc<CancelSignal>,
) -> HttpResult<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(relay.part_path(id))
        .await
        .map_err(anyhow::Error::from)?;
    let mut stream = body.into_data_stream();
    let mut received = 0u64;
    let mut hash = Sha256::new();
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        if cancelled.cancelled.load(Ordering::SeqCst) {
            return Err(err(StatusCode::GONE, "传输已经取消"));
        }
        let chunk = tokio::select! {
            _ = cancelled.wake.notified() => return Err(err(StatusCode::GONE, "传输已经取消")),
            result = tokio::time::timeout(Duration::from_secs(60), stream.next()) => result.map_err(|_| err(StatusCode::REQUEST_TIMEOUT, "上传超过 60 秒未收到数据"))?,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| err(StatusCode::BAD_REQUEST, "上传连接中断"))?;
        if cancelled.cancelled.load(Ordering::SeqCst) {
            return Err(err(StatusCode::GONE, "传输已经取消"));
        }
        let next = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "载荷过长"))?;
        if next > total {
            return Err(err(StatusCode::BAD_REQUEST, "实际载荷超过声明长度"));
        }
        {
            let state = relay.state.lock().unwrap();
            let reserved = state
                .active
                .values()
                .try_fold(0u64, |n, a| n.checked_add(a.remaining))
                .unwrap_or(u64::MAX);
            let required = reserved.checked_add(relay.disk_reserve).unwrap_or(u64::MAX);
            if fs2::available_space(&relay.data_dir).map_err(anyhow::Error::from)? < required {
                return Err(err(
                    StatusCode::INSUFFICIENT_STORAGE,
                    "接收时磁盘空间不足，已停止上传",
                ));
            }
        }
        file.write_all(&chunk).await.map_err(|e| {
            err(
                StatusCode::INSUFFICIENT_STORAGE,
                format!("写入载荷失败：{e}"),
            )
        })?;
        hash.update(&chunk);
        received = next;
        let mut state = relay.state.lock().unwrap();
        if let Some(active) = state.active.get_mut(&id) {
            active.remaining = total - received;
        }
        if last_progress.elapsed() >= Duration::from_millis(50) || received == total {
            let _ = state.events.send(ServerEvent::Progress {
                id,
                received_bytes: received,
                total_bytes: total,
            });
            last_progress = Instant::now();
        }
    }
    if received != total {
        return Err(err(StatusCode::BAD_REQUEST, "载荷未完整接收"));
    }
    if hex::encode(hash.finalize()) != expected {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "载荷 SHA-256 校验失败",
        ));
    }
    file.flush().await.map_err(anyhow::Error::from)?;
    file.sync_all().await.map_err(anyhow::Error::from)?;
    Ok(())
}

struct DownloadLease {
    relay: Arc<Relay>,
    id: Uuid,
}
impl Drop for DownloadLease {
    fn drop(&mut self) {
        let mut state = self.relay.state.lock().unwrap();
        if let Some(count) = state.downloads.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                state.downloads.remove(&self.id);
            }
        }
        self.relay.cleanup_best_effort(&state);
    }
}
struct DownloadStream {
    // Drop the file before releasing the lease, including on disconnect.
    reader: Option<ReaderStream<tokio::fs::File>>,
    lease: Option<DownloadLease>,
}
impl futures_util::Stream for DownloadStream {
    type Item = std::io::Result<bytes::Bytes>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let Some(reader) = self.reader.as_mut() else {
            return std::task::Poll::Ready(None);
        };
        let result = std::pin::Pin::new(reader).poll_next(cx);
        if matches!(result, std::task::Poll::Ready(None | Some(Err(_)))) {
            self.reader.take();
            self.lease.take();
        }
        result
    }
}

async fn download(State(relay): State<Arc<Relay>>, Path(id): Path<Uuid>) -> HttpResult<Response> {
    let (item, file) = {
        let mut state = relay.state.lock().unwrap();
        let item =
            get_item(&state.db, id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "记录不存在"))?;
        if !Relay::retained(&state, id)? {
            return Err(err(
                StatusCode::GONE,
                "记录内容已过期，服务器仅保留最近 10 条",
            ));
        }
        if item.status != ItemStatus::Complete {
            return Err(err(StatusCode::CONFLICT, "载荷尚未完成"));
        }
        // Open and acquire a lease atomically with retention admission. Avoid an
        // async open that could outlive cancellation and race the collector.
        let file = fs::File::open(relay.payload_path(id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                err(StatusCode::GONE, "记录内容已不可用，请重新复制")
            } else {
                HttpError::from(anyhow::Error::from(error))
            }
        })?;
        *state.downloads.entry(id).or_default() += 1;
        (item, file)
    };
    let stream = DownloadStream {
        reader: Some(ReaderStream::with_capacity(
            tokio::fs::File::from_std(file),
            64 * 1024,
        )),
        lease: Some(DownloadLease { relay, id }),
    };
    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&item.bytes.unwrap_or(0).to_string()).unwrap(),
    );
    headers.insert(
        PAYLOAD_SIZE_HEADER,
        HeaderValue::from_str(&item.bytes.unwrap_or(0).to_string()).unwrap(),
    );
    headers.insert(
        PAYLOAD_HASH_HEADER,
        HeaderValue::from_str(item.sha256.as_deref().unwrap_or("")).unwrap(),
    );
    Ok(response)
}
async fn events(State(relay): State<Arc<Relay>>, ws: WebSocketUpgrade) -> HttpResult<Response> {
    let (snapshot, subscription) = relay.snapshot()?;
    Ok(ws.on_upgrade(move |socket| event_socket(socket, snapshot, subscription)))
}
async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> bool {
    let Ok(json) = serde_json::to_string(event) else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            Duration::from_secs(15),
            socket.send(Message::Text(json.into()))
        )
        .await,
        Ok(Ok(()))
    )
}
async fn event_socket(
    mut socket: WebSocket,
    snapshot: ServerEvent,
    mut subscription: broadcast::Receiver<ServerEvent>,
) {
    if !send_event(&mut socket, &snapshot).await {
        return;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            message = subscription.recv() => match message {
                Ok(event) => if !send_event(&mut socket, &event).await { return; },
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    send_event(&mut socket, &ServerEvent::ResyncRequired).await;
                    let _ = socket.close().await;
                    return;
                },
                Err(_) => return,
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                _ => {},
            },
            _ = heartbeat.tick() => {
                if !matches!(tokio::time::timeout(Duration::from_secs(15), socket.send(Message::Ping(Vec::new().into()))).await, Ok(Ok(()))) { return; }
            }
        }
    }
}

pub async fn serve(data_dir: impl AsRef<FsPath>, address: SocketAddr) -> Result<()> {
    let relay = Relay::open(data_dir)?;
    serve_relay(relay, address).await
}
pub async fn serve_relay(relay: Arc<Relay>, address: SocketAddr) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &relay.identity.certificate_path,
        &relay.identity.private_key_path,
    )
    .await?;
    tracing::info!(%address, server_id = %relay.identity.pairing.server_id, "EasyCopy HTTPS relay ready");
    axum_server::bind_rustls(address, config)
        .serve(router(relay).into_make_service())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use easycopy_protocol::ContentKind;

    fn intent() -> RegisterItem {
        RegisterItem {
            id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            device_name: "test".into(),
            kind: ContentKind::Text,
            created_at: 1,
            preview: "test".into(),
        }
    }

    fn upload_headers(payload: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            PAYLOAD_SIZE_HEADER,
            payload.len().to_string().parse().unwrap(),
        );
        headers.insert(
            PAYLOAD_HASH_HEADER,
            hex::encode(Sha256::digest(payload)).parse().unwrap(),
        );
        headers
    }

    async fn complete(relay: &Arc<Relay>, id: Uuid, payload: &'static [u8]) -> Item {
        upload(
            State(relay.clone()),
            Path(id),
            upload_headers(payload),
            Body::from(payload),
        )
        .await
        .unwrap()
        .0
    }

    #[tokio::test]
    async fn retention_counts_intents_not_completions_and_preserves_idempotency() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let request = intent();
        let first = relay.register(request.clone()).unwrap();
        let first = complete(&relay, first.id, b"first").await;
        let mut ids = vec![first.id];
        for _ in 1..HISTORY_LIMIT {
            let item = relay.register(intent()).unwrap();
            complete(&relay, item.id, b"later").await;
            ids.push(item.id);
        }
        assert!(relay.payload_path(first.id).exists());
        // A gap in sequence must not reduce the retained record count.
        relay
            .state
            .lock()
            .unwrap()
            .db
            .execute("UPDATE sqlite_sequence SET seq=100 WHERE name='items'", [])
            .unwrap();
        let newest = relay.register(intent()).unwrap();
        assert_eq!(newest.sequence, 101);
        assert!(!relay.payload_path(first.id).exists());
        assert!(ids[1..].iter().all(|id| relay.payload_path(*id).exists()));
        assert_eq!(relay.register(request.clone()).unwrap(), first);
        assert_eq!(complete(&relay, first.id, b"first").await, first);
        assert!(!relay.payload_path(first.id).exists());
        assert_eq!(
            upload(
                State(relay.clone()),
                Path(first.id),
                upload_headers(b"other"),
                Body::from("other")
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        let mut conflict = request;
        conflict.preview = "different intent".into();
        assert_eq!(
            relay.register(conflict).unwrap_err().0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            download(State(relay.clone()), Path(first.id))
                .await
                .unwrap_err()
                .0,
            StatusCode::GONE
        );
        let (ServerEvent::Snapshot { items, high_water }, _) = relay.snapshot().unwrap() else {
            panic!()
        };
        assert_eq!(high_water, 101);
        assert_eq!(items.len(), HISTORY_LIMIT);
        assert_eq!(items.last().unwrap().sequence, 2);
        assert_eq!(relay.register(intent()).unwrap().sequence, 102);
    }

    #[tokio::test]
    async fn in_flight_downloads_are_pinned_until_eof_or_disconnect() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let item = relay.register(intent()).unwrap();
        complete(&relay, item.id, b"download survives eviction").await;
        let response = download(State(relay.clone()), Path(item.id)).await.unwrap();
        let abandoned = download(State(relay.clone()), Path(item.id)).await.unwrap();
        for _ in 0..HISTORY_LIMIT {
            relay.register(intent()).unwrap();
        }
        assert!(relay.payload_path(item.id).exists());
        assert_eq!(
            download(State(relay.clone()), Path(item.id))
                .await
                .unwrap_err()
                .0,
            StatusCode::GONE
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"download survives eviction");
        assert!(relay.payload_path(item.id).exists());
        assert_eq!(relay.state.lock().unwrap().downloads[&item.id], 1);
        drop(abandoned);
        assert!(!relay.payload_path(item.id).exists());
        assert!(relay.state.lock().unwrap().downloads.is_empty());
    }

    async fn held_upload(
        relay: &Arc<Relay>,
        id: Uuid,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<HttpResult<Json<Item>>>,
    ) {
        let (send, receive) = tokio::sync::oneshot::channel();
        let stream = futures_util::stream::once(async {
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"a"))
        })
        .chain(futures_util::stream::once(async move {
            receive.await.unwrap();
            Ok(bytes::Bytes::from_static(b"b"))
        }));
        let relay2 = relay.clone();
        let task = tokio::spawn(async move {
            upload(
                State(relay2),
                Path(id),
                upload_headers(b"ab"),
                Body::from_stream(stream),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if relay
                    .state
                    .lock()
                    .unwrap()
                    .active
                    .get(&id)
                    .is_some_and(|a| a.remaining == 1)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        (send, task)
    }

    #[tokio::test]
    async fn late_upload_cannot_publish_or_resurrect_expired_payload() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let first = relay.register(intent()).unwrap();
        let (finish, task) = held_upload(&relay, first.id).await;
        for _ in 0..HISTORY_LIMIT {
            let newer = relay.register(intent()).unwrap();
            complete(&relay, newer.id, b"new").await;
        }
        assert!(relay.part_path(first.id).exists());
        let (_, mut events) = relay.snapshot().unwrap();
        finish.send(()).unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().0, StatusCode::GONE);
        assert!(!relay.part_path(first.id).exists());
        assert!(!relay.payload_path(first.id).exists());
        assert_eq!(
            relay.item(first.id).unwrap().unwrap().status,
            ItemStatus::Failed
        );
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event, ServerEvent::Completed { .. }));
        }
        assert_eq!(
            upload(
                State(relay.clone()),
                Path(first.id),
                upload_headers(b"ab"),
                Body::from("ab")
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::GONE
        );
        assert_eq!(
            fs::read_dir(dir.path().join("payloads")).unwrap().count(),
            HISTORY_LIMIT
        );
    }

    #[tokio::test]
    async fn aborted_expired_upload_releases_its_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let item = relay.register(intent()).unwrap();
        let (_finish, task) = held_upload(&relay, item.id).await;
        for _ in 0..HISTORY_LIMIT {
            relay.register(intent()).unwrap();
        }
        assert!(relay.part_path(item.id).exists());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!relay.part_path(item.id).exists());
        assert!(relay.state.lock().unwrap().active.is_empty());
    }

    #[test]
    fn restart_sweeps_legacy_backlog_orphans_and_interrupted_publication() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let pairing = relay.pairing().encode().unwrap();
        let mut items = Vec::new();
        for _ in 0..15 {
            items.push(relay.register(intent()).unwrap());
        }
        // Simulate the old server, including crash after rename but before commit.
        for item in &mut items {
            item.status = ItemStatus::Complete;
            put_item(&relay.state.lock().unwrap().db, item).unwrap();
            fs::write(relay.payload_path(item.id), b"legacy").unwrap();
        }
        items[14].status = ItemStatus::Uploading;
        put_item(&relay.state.lock().unwrap().db, &items[14]).unwrap();
        fs::write(relay.part_path(items[13].id), b"orphan part").unwrap();
        fs::write(relay.payload_path(Uuid::new_v4()), b"orphan payload").unwrap();
        fs::write(dir.path().join("payloads/broken.payload"), b"bad filename").unwrap();
        fs::write(dir.path().join("payloads/keep.txt"), b"unmanaged").unwrap();
        drop(relay);
        let relay = Relay::open(dir.path()).unwrap();
        assert_eq!(relay.pairing().encode().unwrap(), pairing);
        for item in &items {
            assert_eq!(
                relay.payload_path(item.id).exists(),
                (6..=14).contains(&item.sequence)
            );
        }
        assert_eq!(
            relay.item(items[14].id).unwrap().unwrap().status,
            ItemStatus::Failed
        );
        assert!(!relay.part_path(items[13].id).exists());
        assert_eq!(
            fs::read_dir(dir.path().join("payloads")).unwrap().count(),
            10
        );
        assert!(dir.path().join("payloads/keep.txt").exists());
        assert_eq!(relay.register(intent()).unwrap().sequence, 16);
    }

    #[tokio::test]
    async fn missing_content_returns_gone_and_cleanup_failure_is_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let item = relay.register(intent()).unwrap();
        complete(&relay, item.id, b"gone").await;
        fs::remove_file(relay.payload_path(item.id)).unwrap();
        assert_eq!(
            download(State(relay.clone()), Path(item.id))
                .await
                .unwrap_err()
                .0,
            StatusCode::GONE
        );
        let obstructed = dir.path().join("payloads/obstructed.payload");
        fs::create_dir(&obstructed).unwrap();
        let orphan = relay.payload_path(Uuid::new_v4());
        fs::write(&orphan, b"orphan").unwrap();
        assert!(relay
            .cleanup_payloads(&relay.state.lock().unwrap())
            .is_err());
        assert!(
            !orphan.exists(),
            "one failure must not prevent other deletions"
        );
        fs::remove_dir(&obstructed).unwrap();
        fs::write(&obstructed, b"retry after obstruction removed").unwrap();
        // A retry also runs without further registrations/transfers.
        relay.start_cleanup(Duration::from_millis(10));
        tokio::time::timeout(Duration::from_secs(5), async {
            while obstructed.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!obstructed.exists());
    }

    #[test]
    fn restart_keeps_order_and_pairing_but_invalidates_partial_upload() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        let pairing = relay.pairing().encode().unwrap();
        let mut item = relay.register(intent()).unwrap();
        item.status = ItemStatus::Uploading;
        put_item(&relay.state.lock().unwrap().db, &item).unwrap();
        fs::write(relay.part_path(item.id), b"partial").unwrap();
        // A second daemon may not mutate the same metadata/payload directory.
        assert!(Relay::open(dir.path()).is_err());
        drop(relay);
        let relay = Relay::open(dir.path()).unwrap();
        assert_eq!(pairing, relay.pairing().encode().unwrap());
        assert_eq!(
            relay.item(item.id).unwrap().unwrap().status,
            ItemStatus::Failed
        );
        assert!(!relay.part_path(item.id).exists());
        assert_eq!(
            relay.register(intent()).unwrap().sequence,
            item.sequence + 1
        );
    }

    #[test]
    fn snapshot_boundary_never_loses_a_racing_registration() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::open(dir.path()).unwrap();
        for _ in 0..32 {
            let request = intent();
            let id = request.id;
            let writer = relay.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let start = barrier.clone();
            let thread = std::thread::spawn(move || {
                start.wait();
                writer.register(request).unwrap()
            });
            barrier.wait();
            let (snapshot, mut events) = relay.snapshot().unwrap();
            let record = thread.join().unwrap();
            let ServerEvent::Snapshot { items, high_water } = snapshot else {
                panic!()
            };
            let in_snapshot = items.iter().any(|item| item.id == id);
            let in_live =
                matches!(events.try_recv(), Ok(ServerEvent::Registered { item }) if item.id == id);
            assert_ne!(in_snapshot, in_live);
            assert_eq!(in_snapshot, record.sequence <= high_water);
        }
    }
}
