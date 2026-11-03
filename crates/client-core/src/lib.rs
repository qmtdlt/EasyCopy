pub mod archive;
pub mod clipboard;
mod sync;
pub mod transport;
mod upload_confirmation;
pub use upload_confirmation::{FileUploadPrompt, FILE_CONFIRM_BYTES};

use anyhow::{bail, Context, Result};
use base64::Engine;
use clipboard::NativeClipboard;
pub use clipboard::{Clipboard, ClipboardContent, MemoryClipboard};
use easycopy_protocol::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{broadcast, mpsc, Notify};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(default)]
    pub available: bool,
    pub id: Uuid,
    pub kind: ContentKind,
    pub device_name: String,
    pub created_at: i64,
    pub sequence: Option<u64>,
    pub preview: String,
    pub detail: String,
    pub thumbnail: Option<String>,
    pub status: String,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    pub address: String,
    pub port: u16,
    pub pairing_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettings {
    pub address: String,
    pub port: u16,
    pub has_pairing_key: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferEvent {
    pub task_id: String,
    pub direction: String,
    pub phase: String,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}
#[derive(Debug, Clone)]
pub enum ClientEvent {
    HistoryChanged,
    Connection { connected: bool, message: String },
    Transfer(TransferEvent),
    FileUploadPrompt(Option<FileUploadPrompt>),
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    file_size_limit: Option<u64>,
    #[serde(default)]
    server_id: Option<Uuid>,
    #[serde(default)]
    capture_saved: bool,
    #[serde(default)]
    download_attempts: u8,
    #[serde(default)]
    receive_generation: u64,
    #[serde(default)]
    receive_epoch: u64,
    #[serde(default)]
    auto_apply: bool,
    #[serde(default)]
    source_refs: Vec<PathBuf>,
    entry: HistoryEntry,
    item: Option<Item>,
    #[serde(default)]
    ordering_sequence: u64,
    local_order: u64,
    prepared: bool,
    files: Vec<PathBuf>,
    retries: u8,
    pending_upload: bool,
    unreferenced_at: Option<i64>,
}
#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    records: HashMap<Uuid, Record>,
    #[serde(default)]
    history: Vec<Uuid>,
    #[serde(default)]
    current: Option<Uuid>,
    #[serde(default)]
    order: u64,
}
#[derive(Serialize, Deserialize)]
struct Configuration {
    device_id: Uuid,
    device_name: String,
    address: String,
    port: u16,
    #[serde(default = "upload_confirmation::default_threshold_mb")]
    file_upload_threshold_mb: u64,
}
enum CredentialState {
    Loading,
    Ready,
    Failed(String),
}
struct State {
    saved: Persisted,
    config: Configuration,
    pairing: Option<Pairing>,
    credential_state: CredentialState,
    settings_epoch: u64,
    transport: Option<transport::Transport>,
    connected: bool,
    epoch: u64,
    high_water: u64,
    latest_sequence: u64,
    local_generation: u64,
    native_sequence: u64,
    pending_file_copy: Option<upload_confirmation::PendingFileCopy>,
    pending_local: HashSet<Uuid>,
    in_flight: HashSet<Uuid>,
}
struct LocalCopy {
    id: Uuid,
    content: ClipboardContent,
    transport: Option<transport::Transport>,
}

pub struct ClientCore {
    directory: PathBuf,
    clipboard: Arc<dyn Clipboard>,
    state: Mutex<State>,
    events: broadcast::Sender<ClientEvent>,
    local_tx: mpsc::UnboundedSender<LocalCopy>,
    local_rx: Mutex<Option<mpsc::UnboundedReceiver<LocalCopy>>>,
    reconnect: Notify,
    stopped: AtomicBool,
    started: AtomicBool,
    memory_credentials: bool,
    saving: Mutex<HashMap<Uuid, usize>>,
    capture_serial: Mutex<()>,
}
impl ClientCore {
    pub fn open(directory: PathBuf) -> Result<Arc<Self>> {
        Self::open_internal(directory, Arc::new(NativeClipboard::default()), false)
    }
    pub fn open_with_clipboard(
        directory: PathBuf,
        clipboard: Arc<dyn Clipboard>,
    ) -> Result<Arc<Self>> {
        Self::open_internal(directory, clipboard, true)
    }
    fn open_internal(
        directory: PathBuf,
        clipboard: Arc<dyn Clipboard>,
        memory_credentials: bool,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(directory.join("cache"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let configuration = directory.join("settings.json");
        let config = if configuration.exists() {
            serde_json::from_slice(&std::fs::read(configuration)?)?
        } else {
            Configuration {
                device_id: Uuid::new_v4(),
                device_name: hostname::get()?.to_string_lossy().into_owned(),
                address: String::new(),
                port: 8443,
                file_upload_threshold_mb: upload_confirmation::default_threshold_mb(),
            }
        };
        let history = directory.join("history.json");
        let mut saved: Persisted = if history.exists() {
            serde_json::from_slice(&std::fs::read(history)?).context("本地历史文件损坏")?
        } else {
            Persisted::default()
        };
        // Opening the app must never invoke SecurityAgent on its main thread.
        // System credential access starts later on Tokio's blocking pool.
        // A process restart cannot assume its former clipboard write is still current.
        // Live native file references are additionally checked by GC itself.
        saved.current = None;
        let native_sequence = clipboard.sequence()?;
        let (events, _) = broadcast::channel(256);
        let (local_tx, local_rx) = mpsc::unbounded_channel();
        let core = Arc::new(Self {
            directory,
            clipboard,
            state: Mutex::new(State {
                saved,
                config,
                pairing: None,
                credential_state: if memory_credentials {
                    CredentialState::Ready
                } else {
                    CredentialState::Loading
                },
                settings_epoch: 0,
                transport: None,
                connected: false,
                epoch: 0,
                high_water: 0,
                latest_sequence: 0,
                local_generation: 0,
                native_sequence,
                pending_file_copy: None,
                pending_local: HashSet::new(),
                in_flight: HashSet::new(),
            }),
            events,
            local_tx,
            local_rx: Mutex::new(Some(local_rx)),
            reconnect: Notify::new(),
            stopped: AtomicBool::new(false),
            started: AtomicBool::new(false),
            memory_credentials,
            saving: Mutex::new(HashMap::new()),
            capture_serial: Mutex::new(()),
        });
        core.persist()?;
        Ok(core)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<ClientEvent> {
        self.events.subscribe()
    }
    pub fn history(&self) -> Vec<HistoryEntry> {
        let state = self.state.lock().unwrap();
        state
            .saved
            .history
            .iter()
            .filter_map(|id| {
                state.saved.records.get(id).map(|r| {
                    let mut entry = r.entry.clone();
                    entry.available = r.prepared;
                    entry
                })
            })
            .collect()
    }
    pub fn settings(&self) -> PublicSettings {
        let state = self.state.lock().unwrap();
        PublicSettings {
            address: state.config.address.clone(),
            port: state.config.port,
            has_pairing_key: state.pairing.is_some(),
        }
    }
    pub fn device_id(&self) -> Uuid {
        self.state.lock().unwrap().config.device_id
    }
    pub fn is_connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }
    pub async fn test_connection(&self, input: SettingsInput) -> Result<ServerInfo> {
        let pairing = self.resolve_pairing(&input.pairing_key)?;
        transport::Transport::new(&input.address, input.port, pairing)
            .await?
            .info()
            .await
    }
    fn resolve_pairing(&self, value: &str) -> Result<Pairing> {
        if value.trim().is_empty() {
            self.state
                .lock()
                .unwrap()
                .pairing
                .clone()
                .context("请填写连接密钥")
        } else {
            Pairing::decode(value)
        }
    }
    pub async fn save_settings(self: &Arc<Self>, input: SettingsInput) -> Result<()> {
        let pairing = self.resolve_pairing(&input.pairing_key)?;
        let transport =
            transport::Transport::new(&input.address, input.port, pairing.clone()).await?;
        transport.info().await?;
        if !self.memory_credentials {
            let account = self.device_id().to_string();
            let encoded = pairing.encode()?;
            tokio::task::spawn_blocking(move || -> Result<()> {
                keyring::Entry::new("EasyCopy", &account)?
                    .set_password(&encoded)
                    .context("无法保存到系统凭据存储")
            })
            .await
            .context("系统凭据保存任务失败")??;
        }
        {
            let mut state = self.state.lock().unwrap();
            state.config.address = input.address.trim().to_owned();
            state.config.port = input.port;
            let changed_server = state
                .pairing
                .as_ref()
                .is_some_and(|old| old.server_id != pairing.server_id);
            if changed_server {
                for record in state.saved.records.values_mut() {
                    if record.pending_upload {
                        record.pending_upload = false;
                        record.entry.error =
                            Some("已切换服务器，旧任务停止同步；可从历史重新复制".into());
                        record.entry.status = "failed".into();
                    }
                    record.ordering_sequence = 0;
                }
                state.latest_sequence = 0;
                state.high_water = 0;
                state.local_generation += 1;
            }
            state.settings_epoch += 1;
            state.credential_state = CredentialState::Ready;
            state.pairing = Some(pairing);
            state.transport = Some(transport);
            state.connected = false;
            state.epoch += 1;
        }
        self.persist()?;
        self.reconnect.notify_one();
        Ok(())
    }
    pub async fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let core = self.clone();
        tokio::spawn(async move {
            core.connection_loop().await;
        });
        let core = self.clone();
        tokio::spawn(async move {
            core.local_loop().await;
        });
        let core = self.clone();
        tokio::spawn(async move {
            core.monitor_loop().await;
        });
        let core = self.clone();
        tokio::spawn(async move {
            while !core.stopped.load(Ordering::SeqCst) {
                if let Err(error) = core.cleanup() {
                    tracing::warn!(%error, "cache cleanup failed");
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
        if !self.memory_credentials {
            let core = self.clone();
            tokio::spawn(async move {
                core.load_credentials_with(|account| {
                    match keyring::Entry::new("EasyCopy", &account)?.get_password() {
                        Ok(value) => Ok(Some(
                            Pairing::decode(&value).context("已保存的连接密钥无效，请重新填写")?,
                        )),
                        Err(keyring::Error::NoEntry) => Ok(None),
                        Err(error) => Err(error)
                            .context("无法读取系统凭据存储；请检查系统授权提示或重新填写连接密钥"),
                    }
                })
                .await;
            });
        }
        self.clone().recover_preparations().await;
    }
    async fn load_credentials_with(
        self: Arc<Self>,
        reader: impl FnOnce(String) -> Result<Option<Pairing>> + Send + 'static,
    ) {
        let (account, epoch) = {
            let mut state = self.state.lock().unwrap();
            if state.pairing.is_some() || self.stopped.load(Ordering::SeqCst) {
                return;
            }
            state.credential_state = CredentialState::Loading;
            (state.config.device_id.to_string(), state.settings_epoch)
        };
        let _ = self.events.send(ClientEvent::Connection {
            connected: false,
            message: "正在读取系统凭据；如出现系统授权提示，请在提示中处理。本地历史仍可使用。"
                .into(),
        });
        let result = tokio::task::spawn_blocking(move || reader(account))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
        self.finish_credential_load(epoch, result);
    }
    fn finish_credential_load(&self, epoch: u64, result: Result<Option<Pairing>>) {
        let message = {
            let mut state = self.state.lock().unwrap();
            // A settings save owns the new epoch. A delayed OS dialog must not
            // replace that configuration, even if the keychain read began first.
            if self.stopped.load(Ordering::SeqCst) || state.settings_epoch != epoch {
                return;
            }
            match result {
                Ok(pairing) => {
                    let message = if pairing.is_some() {
                        "系统凭据已读取，正在连接"
                    } else {
                        "请先设置中转服务器"
                    }
                    .to_string();
                    state.pairing = pairing;
                    state.credential_state = CredentialState::Ready;
                    message
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    state.credential_state = CredentialState::Failed(message.clone());
                    message
                }
            }
        };
        let _ = self.events.send(ClientEvent::Connection {
            connected: false,
            message,
        });
        self.reconnect.notify_one();
    }
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.reconnect.notify_waiters();
    }
    fn is_pairing_content(content: &ClipboardContent) -> bool {
        matches!(content, ClipboardContent::Text(text) if text.trim().starts_with(PAIRING_PREFIX))
    }
    /// Explicit user copy intent, useful for headless integration and history-copy actions.
    pub async fn submit_content(self: &Arc<Self>, content: ClipboardContent) -> Result<Uuid> {
        if Self::is_pairing_content(&content) {
            bail!("连接密钥仅用于设置，不进入剪贴板历史");
        }
        let _serial = self.capture_serial.lock().unwrap();
        let sequence = self.clipboard.sequence()?;
        let written = self
            .clipboard
            .write_if_sequence(&content, sequence)?
            .context("复制期间剪贴板发生变化，请重试")?;
        self.state.lock().unwrap().native_sequence = written;
        self.accept_local_content(content, written)
    }
    pub async fn copy_item(self: &Arc<Self>, id: Uuid) -> Result<()> {
        let content = self.content(id)?;
        if matches!(content, ClipboardContent::Files(_)) {
            bail!("文件仅支持另存为，不能从历史记录再次复制");
        }
        self.submit_content(content).await?;
        Ok(())
    }
    pub async fn save_item(&self, id: Uuid, destination: PathBuf) -> Result<()> {
        let record = {
            let state = self.state.lock().unwrap();
            let record = state
                .saved
                .records
                .get(&id)
                .cloned()
                .context("历史记录已不存在")?;
            if !record.prepared {
                bail!("内容尚未完整下载或准备完成");
            }
            *self.saving.lock().unwrap().entry(id).or_default() += 1;
            record
        };
        let source = self.payload_path(id);
        let result = tokio::task::spawn_blocking(move || {
            if record.entry.kind == ContentKind::Files {
                if !destination.is_dir() {
                    bail!("请选择保存目录");
                }
                for path in record.files {
                    archive::copy_saved(
                        &path,
                        &destination.join(path.file_name().context("文件名无效")?),
                    )?;
                }
            } else {
                archive::copy_saved(&source, &destination)?;
            }
            Ok(())
        })
        .await;
        {
            let mut saving = self.saving.lock().unwrap();
            if let Some(count) = saving.get_mut(&id) {
                *count -= 1;
                if *count == 0 {
                    saving.remove(&id);
                }
            }
        }
        result?
    }
    fn content(&self, id: Uuid) -> Result<ClipboardContent> {
        let record = self
            .state
            .lock()
            .unwrap()
            .saved
            .records
            .get(&id)
            .cloned()
            .context("历史记录已不存在")?;
        if !record.prepared {
            bail!("内容尚未准备完成");
        }
        Ok(match record.entry.kind {
            ContentKind::Files => ClipboardContent::Files(record.files),
            ContentKind::Text => {
                ClipboardContent::Text(std::fs::read_to_string(self.payload_path(id))?)
            }
            ContentKind::Image => ClipboardContent::Image(std::fs::read(self.payload_path(id))?),
        })
    }
    fn save_capture(&self, id: Uuid, content: &ClipboardContent) -> Result<()> {
        let path = self.payload_path(id).with_file_name("capture");
        std::fs::create_dir_all(path.parent().unwrap())?;
        let bytes: &[u8] = match content {
            ClipboardContent::Text(text) => text.as_bytes(),
            ClipboardContent::Image(bytes) => bytes,
            ClipboardContent::Files(_) => &[],
        };
        archive::ensure_space(path.parent().unwrap(), bytes.len() as u64)?;
        archive::write_checked(&path, bytes)?;
        self.state
            .lock()
            .unwrap()
            .saved
            .records
            .get_mut(&id)
            .context("捕获记录丢失")?
            .capture_saved = true;
        self.persist()
    }
    fn load_capture(&self, id: Uuid) -> Result<ClipboardContent> {
        let record = self
            .state
            .lock()
            .unwrap()
            .saved
            .records
            .get(&id)
            .cloned()
            .context("捕获记录丢失")?;
        if !record.capture_saved {
            bail!("准备被中断且原始捕获不可用，请重新复制");
        }
        let path = self.payload_path(id).with_file_name("capture");
        Ok(match record.entry.kind {
            ContentKind::Text => ClipboardContent::Text(std::fs::read_to_string(path)?),
            ContentKind::Image => ClipboardContent::Image(std::fs::read(path)?),
            ContentKind::Files => ClipboardContent::Files(record.source_refs),
        })
    }
    fn payload_path(&self, id: Uuid) -> PathBuf {
        self.directory
            .join("cache")
            .join(id.to_string())
            .join("payload")
    }
    fn persist(&self) -> Result<()> {
        let state = self.state.lock().unwrap();
        Self::atomic_json(&self.directory.join("history.json"), &state.saved)?;
        Self::atomic_json(&self.directory.join("settings.json"), &state.config)
    }
    fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
        use std::io::Write;
        let temp = path.with_extension("tmp");
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::ffi::OsStrExt;
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn MoveFileExW(existing: *const u16, destination: *const u16, flags: u32) -> i32;
            }
            let source: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
            let destination: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(target_os = "windows"))]
        std::fs::rename(temp, path)?;
        Ok(())
    }
    fn transfer(
        &self,
        id: Uuid,
        direction: &str,
        phase: &str,
        received: u64,
        total: Option<u64>,
        error: Option<String>,
    ) {
        let _ = self.events.send(ClientEvent::Transfer(TransferEvent {
            task_id: format!("{direction}:{id}"),
            direction: direction.into(),
            phase: phase.into(),
            received_bytes: received,
            total_bytes: total,
            error,
        }));
    }
    fn refresh_history(state: &mut State) {
        let mut records: Vec<_> = state
            .saved
            .records
            .values()
            .filter(|r| r.unreferenced_at.is_none() || state.saved.history.contains(&r.entry.id))
            .collect();
        // Server sequence defines cross-device order. Unsynchronized local copies use
        // their arrival order; server items are inserted only once, never on completion.
        records.sort_by(|a, b| {
            b.ordering_sequence
                .cmp(&a.ordering_sequence)
                .then_with(|| b.local_order.cmp(&a.local_order))
        });
        state.saved.history = records
            .into_iter()
            .take(HISTORY_LIMIT)
            .map(|r| r.entry.id)
            .collect();
    }
    fn thumbnail(bytes: &[u8]) -> Option<String> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .ok()?
            .thumbnail(180, 110);
        let mut buffer = std::io::Cursor::new(Vec::new());
        image.write_to(&mut buffer, image::ImageFormat::Png).ok()?;
        Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn restart_recovers_offline_capture_without_replaying_it() {
        let root = tempfile::tempdir().unwrap();
        let clipboard = Arc::new(MemoryClipboard::default());
        let core = ClientCore::open_with_clipboard(root.path().into(), clipboard.clone()).unwrap();
        let id = core
            .submit_content(ClipboardContent::Text("persistent offline copy".into()))
            .await
            .unwrap();
        assert!(!core.history()[0].available);
        drop(core);
        let restored = ClientCore::open_with_clipboard(root.path().into(), clipboard).unwrap();
        restored.start().await;
        let history = restored.history();
        assert_eq!(history[0].id, id);
        assert!(history[0].available);
        assert_eq!(history[0].status, "local");
        assert!(restored.state.lock().unwrap().saved.records[&id]
            .item
            .is_none());
        restored.shutdown();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_system_credentials_leave_local_copy_and_history_usable() {
        let root = tempfile::tempdir().unwrap();
        let core = ClientCore::open_with_clipboard(
            root.path().into(),
            Arc::new(MemoryClipboard::default()),
        )
        .unwrap();
        core.start().await;
        let (release, waiting) = std::sync::mpsc::channel();
        let reader_started = Arc::new(AtomicBool::new(false));
        let signal = reader_started.clone();
        let loader = tokio::spawn(core.clone().load_credentials_with(move |_| {
            signal.store(true, Ordering::SeqCst);
            waiting.recv().unwrap();
            Err(anyhow::anyhow!("测试：系统授权被拒绝"))
        }));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !reader_started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            let id = core
                .submit_content(ClipboardContent::Text("钥匙串等待期间仍可本地复制".into()))
                .await
                .unwrap();
            while !core.history().iter().any(|r| r.id == id && r.available) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!core.settings().has_pairing_key);
        release.send(()).unwrap();
        loader.await.unwrap();
        assert!(
            matches!(&core.state.lock().unwrap().credential_state, CredentialState::Failed(message) if message.contains("系统授权被拒绝"))
        );
        assert!(core.history()[0].available);
        core.shutdown();
    }
    #[test]
    fn delayed_credential_result_cannot_replace_new_settings() {
        let root = tempfile::tempdir().unwrap();
        let core = ClientCore::open_with_clipboard(
            root.path().into(),
            Arc::new(MemoryClipboard::default()),
        )
        .unwrap();
        let server_id = Uuid::new_v4();
        {
            let mut state = core.state.lock().unwrap();
            state.settings_epoch = 1;
            state.pairing = Some(Pairing {
                version: 1,
                server_id,
                logical_name: format!("relay.{server_id}.invalid"),
                ca_pem: "-----BEGIN CERTIFICATE-----".into(),
                secret: "a".repeat(64),
            });
        }
        core.finish_credential_load(0, Ok(None));
        assert_eq!(
            core.state
                .lock()
                .unwrap()
                .pairing
                .as_ref()
                .unwrap()
                .server_id,
            server_id
        );
        core.finish_credential_load(0, Err(anyhow::anyhow!("迟到的读取错误")));
        assert!(matches!(
            core.state.lock().unwrap().credential_state,
            CredentialState::Ready
        ));
    }
    #[tokio::test]
    async fn native_pairing_strings_never_enter_history_or_registration_queue() {
        let root = tempfile::tempdir().unwrap();
        let clipboard = Arc::new(MemoryClipboard::default());
        let core = ClientCore::open_with_clipboard(root.path().into(), clipboard.clone()).unwrap();
        let mut events = core.subscribe();
        let mut registration_queue = core.local_rx.lock().unwrap().take().unwrap();
        core.state.lock().unwrap().connected = true;
        let monitor = tokio::spawn(core.clone().monitor_loop());
        for text in ["easycopy1:", "  \n easycopy1:invalid-or-truncated  "] {
            let previous_generation = core.state.lock().unwrap().local_generation;
            clipboard.set(ClipboardContent::Text(text.into()));
            let expected_sequence = clipboard.sequence().unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while core.state.lock().unwrap().native_sequence != expected_sequence {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let state = core.state.lock().unwrap();
            assert!(state.local_generation > previous_generation);
            assert!(state.saved.current.is_none());
            assert!(state.saved.records.is_empty());
            assert!(state.pending_local.is_empty());
            drop(state);
            assert!(core.history().is_empty());
            assert!(matches!(
                registration_queue.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            assert!(matches!(
                events.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
        }
        assert_eq!(
            std::fs::read_dir(root.path().join("cache"))
                .unwrap()
                .count(),
            0
        );
        core.shutdown();
        monitor.await.unwrap();
    }
    #[tokio::test]
    async fn explicit_pairing_copy_is_rejected_before_any_clipboard_or_history_change() {
        let root = tempfile::tempdir().unwrap();
        let clipboard = Arc::new(MemoryClipboard::default());
        clipboard.set(ClipboardContent::Text("keep current clipboard".into()));
        let core = ClientCore::open_with_clipboard(root.path().into(), clipboard.clone()).unwrap();
        let mut events = core.subscribe();
        let mut registration_queue = core.local_rx.lock().unwrap().take().unwrap();
        let before_sequence = clipboard.sequence().unwrap();
        let error = core
            .submit_content(ClipboardContent::Text(" \neasycopy1:broken ".into()))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "连接密钥仅用于设置，不进入剪贴板历史");
        assert_eq!(clipboard.sequence().unwrap(), before_sequence);
        assert_eq!(
            clipboard.content(),
            Some(ClipboardContent::Text("keep current clipboard".into()))
        );
        assert!(core.history().is_empty());
        assert!(matches!(
            registration_queue.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
    #[test]
    fn stale_native_sequence_never_replaces_new_copy() {
        let clipboard = MemoryClipboard::default();
        let old = clipboard.sequence().unwrap();
        clipboard.set(ClipboardContent::Text("new local intent".into()));
        assert!(clipboard
            .write_if_sequence(&ClipboardContent::Text("late network result".into()), old)
            .unwrap()
            .is_none());
        assert_eq!(
            clipboard.content(),
            Some(ClipboardContent::Text("new local intent".into()))
        );
    }
}
