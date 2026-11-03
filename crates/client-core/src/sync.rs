use super::*;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

impl ClientCore {
    pub(super) fn capture_locked(
        &self,
        state: &mut State,
        content: &ClipboardContent,
        id: Uuid,
    ) -> Uuid {
        let (kind, preview) = match content {
            ClipboardContent::Text(text) => (ContentKind::Text, text.chars().take(160).collect()),
            ClipboardContent::Image(_) => (ContentKind::Image, "图片".into()),
            ClipboardContent::Files(paths) => (
                ContentKind::Files,
                paths
                    .iter()
                    .filter_map(|p| p.file_name())
                    .map(|n| n.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("、")
                    .chars()
                    .take(160)
                    .collect(),
            ),
        };
        state.local_generation += 1;
        state.saved.order += 1;
        state.saved.current = Some(id);
        state.pending_local.insert(id);
        state.in_flight.insert(id);
        let record = Record {
            file_size_limit: None,
            server_id: if state.connected {
                state.pairing.as_ref().map(|p| p.server_id)
            } else {
                None
            },
            capture_saved: false,
            download_attempts: 0,
            receive_generation: state.local_generation,
            receive_epoch: state.epoch,
            auto_apply: false,
            source_refs: match content {
                ClipboardContent::Files(p) => p.clone(),
                _ => vec![],
            },
            entry: HistoryEntry {
                available: false,
                id,
                kind,
                device_name: state.config.device_name.clone(),
                created_at: chrono::Utc::now().timestamp_millis(),
                sequence: None,
                preview,
                detail: "准备中".into(),
                thumbnail: None,
                status: "preparing".into(),
                error: None,
            },
            item: None,
            local_order: state.saved.order,
            ordering_sequence: state.latest_sequence,
            prepared: false,
            files: vec![],
            retries: 0,
            pending_upload: false,
            unreferenced_at: None,
        };
        state.saved.records.insert(id, record);
        Self::refresh_history(state);
        id
    }
    pub(super) async fn recover_preparations(self: Arc<Self>) {
        let recover: Vec<_> = {
            let mut state = self.state.lock().unwrap();
            let ids: Vec<_> = state
                .saved
                .records
                .values()
                .filter(|r| {
                    !r.prepared
                        && !state.pending_local.contains(&r.entry.id)
                        && (r
                            .item
                            .as_ref()
                            .is_some_and(|i| i.device_id == state.config.device_id)
                            || r.item.is_none())
                })
                .map(|r| r.entry.id)
                .collect();
            for id in &ids {
                state.in_flight.insert(*id);
            }
            ids
        };
        for id in recover {
            let input = self.load_capture(id);
            let outcome = match input {
                Ok(content) => {
                    let core = self.clone();
                    tokio::task::spawn_blocking(move || core.prepare(id, content))
                        .await
                        .unwrap_or_else(|e| Err(e.into()))
                }
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) => {
                    let (pending, connected) = {
                        let state = self.state.lock().unwrap();
                        (state.saved.records[&id].pending_upload, state.connected)
                    };
                    self.state.lock().unwrap().in_flight.remove(&id);
                    if pending && connected {
                        self.clone().upload_record(id).await;
                    } else if !pending {
                        if let Some(r) = self.state.lock().unwrap().saved.records.get_mut(&id) {
                            r.entry.status = "local".into();
                        }
                    }
                }
                Err(error) => {
                    self.fail_record(id, "upload", &format!("准备中断后无法恢复：{error:#}"));
                    let transport = {
                        let mut state = self.state.lock().unwrap();
                        let server_id = state.saved.records.get(&id).and_then(|r| r.server_id);
                        if let Some(r) = state.saved.records.get_mut(&id) {
                            r.pending_upload = false;
                        }
                        state
                            .transport
                            .clone()
                            .filter(|t| Some(t.pairing.server_id) == server_id)
                    };
                    if let Some(transport) = transport {
                        let _ = transport.cancel(id).await;
                    }
                }
            }
            let _ = self.persist();
            let _ = self.events.send(ClientEvent::HistoryChanged);
        }
    }
    pub(super) async fn monitor_loop(self: Arc<Self>) {
        while !self.stopped.load(Ordering::SeqCst) {
            {
                let _serial = self.capture_serial.lock().unwrap();
                let capture = (|| -> Result<()> {
                    let _native_notification = self.clipboard.poll_changed();
                    let sequence = self.clipboard.sequence()?;
                    if sequence == self.state.lock().unwrap().native_sequence {
                        return Ok(());
                    }
                    let content = self.clipboard.read()?;
                    if self.clipboard.sequence()? != sequence {
                        return Ok(());
                    }
                    self.state.lock().unwrap().native_sequence = sequence;
                    // Unsupported content and pairing keys still supersede pending copies.
                    if let Some(content) = content.filter(|c| !Self::is_pairing_content(c)) {
                        self.accept_local_content(content, sequence)?;
                    } else {
                        self.invalidate_file_prompt();
                    }
                    Ok(())
                })();
                match capture {
                    Err(error) => {
                        let _ = self.events.send(ClientEvent::Connection {
                            connected: self.is_connected(),
                            message: format!("读取剪贴板失败：{error}"),
                        });
                    }
                    _ => (),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    pub(super) async fn local_loop(self: Arc<Self>) {
        let mut receiver = self.local_rx.lock().unwrap().take().unwrap();
        while let Some(copy) = receiver.recv().await {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            let registration = {
                let state = self.state.lock().unwrap();
                let record = &state.saved.records[&copy.id];
                RegisterItem {
                    id: copy.id,
                    device_id: state.config.device_id,
                    device_name: state.config.device_name.clone(),
                    kind: record.entry.kind,
                    created_at: record.entry.created_at,
                    preview: record.entry.preview.clone(),
                }
            };
            // Intents are registered serially in local capture order; payload work
            // starts only afterwards and may finish in any order.
            let registered = if let Some(transport) = copy.transport {
                match transport.register(&registration).await {
                    Ok(item) => Some(item),
                    Err(_) => transport.query(copy.id).await.ok(), // POST response may have been lost.
                }
            } else {
                None
            };
            {
                let mut state = self.state.lock().unwrap();
                state.pending_local.remove(&copy.id);
                if let Some(item) = &registered {
                    let belongs = state.saved.records[&copy.id].server_id
                        == state.pairing.as_ref().map(|p| p.server_id);
                    if belongs {
                        state.latest_sequence = state.latest_sequence.max(item.sequence);
                    }
                    if let Some(record) = state.saved.records.get_mut(&copy.id) {
                        record.entry.sequence = Some(item.sequence);
                        if belongs {
                            record.ordering_sequence = item.sequence;
                        }
                        record.item = Some(item.clone());
                        record.pending_upload = belongs;
                    }
                }
                Self::refresh_history(&mut state);
            }
            let _ = self.persist();
            let core = self.clone();
            tokio::spawn(async move {
                core.transfer(copy.id, "upload", "start", 0, None, None);
                core.transfer(copy.id, "upload", "preparing", 0, None, None);
                let core2 = core.clone();
                let id = copy.id;
                let prepared =
                    tokio::task::spawn_blocking(move || core2.prepare(id, copy.content)).await;
                match prepared {
                    Ok(Ok(())) => {
                        if registered.is_some() {
                            core.clone().upload_record(id).await;
                        } else {
                            {
                                let mut state = core.state.lock().unwrap();
                                if let Some(r) = state.saved.records.get_mut(&id) {
                                    r.entry.status = "local".into();
                                    r.entry.detail.push_str(" · 仅本机");
                                }
                                state.in_flight.remove(&id);
                            }
                            core.transfer(
                                id,
                                "upload",
                                "failed",
                                0,
                                None,
                                Some("未连接服务器，已保留本地历史".into()),
                            );
                        }
                    }
                    Ok(Err(error)) => {
                        core.clone()
                            .preparation_failed(id, &format!("{error:#}"))
                            .await
                    }
                    Err(error) => {
                        core.clone()
                            .preparation_failed(id, &error.to_string())
                            .await
                    }
                }
                let _ = core.persist();
                let _ = core.events.send(ClientEvent::HistoryChanged);
            });
        }
    }
    fn prepare(&self, id: Uuid, content: ClipboardContent) -> Result<()> {
        let payload = self.payload_path(id);
        std::fs::create_dir_all(payload.parent().unwrap())?;
        let temporary = payload.with_extension("preparing");
        let _ = std::fs::remove_file(&temporary);
        let mut packed_digest = None;
        let (kind, files, thumbnail, detail) = match content {
            ClipboardContent::Files(paths) => {
                let limit = self.state.lock().unwrap().saved.records[&id].file_size_limit;
                packed_digest = Some(archive::pack_with_limit(&paths, &temporary, limit)?);
                let restore_path = payload.parent().unwrap().join("files");
                if restore_path.exists() {
                    std::fs::remove_dir_all(&restore_path)?;
                }
                let restored = archive::unpack(&temporary, &restore_path)?;
                (
                    ContentKind::Files,
                    restored,
                    None,
                    format!("{} 个项目", paths.len()),
                )
            }
            ClipboardContent::Text(text) => {
                archive::ensure_space(payload.parent().unwrap(), text.len() as u64)?;
                archive::write_checked(&temporary, text.as_bytes())?;
                (ContentKind::Text, vec![], None, "文本".into())
            }
            ClipboardContent::Image(bytes) => {
                archive::ensure_space(payload.parent().unwrap(), bytes.len() as u64)?;
                let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                    .context("剪贴板图片不是有效 PNG")?;
                archive::write_checked(&temporary, &bytes)?;
                (
                    ContentKind::Image,
                    vec![],
                    Self::thumbnail(&bytes),
                    format!("{} × {}", image.width(), image.height()),
                )
            }
        };
        // pack already hashes the complete, synced archive; restoring history
        // only reads it. Reuse that digest instead of scanning the archive twice.
        let (bytes, sha256) = match packed_digest {
            Some(digest) => digest,
            None => archive::hash_file(&temporary)?,
        };
        std::fs::rename(temporary, &payload)?;
        {
            let mut state = self.state.lock().unwrap();
            let record = state.saved.records.get_mut(&id).context("本地记录已丢失")?;
            record.prepared = true;
            record.files = files;
            record.entry.thumbnail = thumbnail;
            record.entry.detail = format!("{detail} · {}", human_bytes(bytes));
            record.entry.status = "ready".into();
            if let Some(item) = &mut record.item {
                item.bytes = Some(bytes);
                item.sha256 = Some(sha256);
            }
            // A local file copy still points at the user's source files. Preserve
            // those original references; history uses the immutable cached copy.
            if kind == ContentKind::Files && record.files.is_empty() {
                bail!("没有还原文件");
            }
        }
        self.persist()?;
        let _ = self.events.send(ClientEvent::HistoryChanged);
        Ok(())
    }
    async fn upload_record(self: Arc<Self>, id: Uuid) {
        let (transport, item, retries) = {
            let mut state = self.state.lock().unwrap();
            let Some(record) = state.saved.records.get(&id) else {
                return;
            };
            let Some(item) = record.item.clone() else {
                return;
            };
            let retries = record.retries;
            let transport = state
                .transport
                .clone()
                .filter(|t| record.server_id == Some(t.pairing.server_id));
            state.in_flight.insert(id);
            (transport, item, retries)
        };
        let Some(transport) = transport else {
            self.fail_record(id, "upload", "服务器身份已改变；旧记录仅保留本地");
            if let Some(r) = self.state.lock().unwrap().saved.records.get_mut(&id) {
                r.pending_upload = false;
            }
            let _ = self.persist();
            return;
        };
        let Some(bytes) = item.bytes else {
            return;
        };
        let Some(sha256) = item.sha256 else {
            return;
        };
        self.transfer(id, "upload", "start", 0, Some(bytes), None);
        self.transfer(id, "upload", "transferring", 0, Some(bytes), None);
        let mut last_error = String::new();
        for attempt in retries..=3 {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            if attempt > retries {
                tokio::time::sleep(Duration::from_millis(500 * (1u64 << attempt))).await;
            }
            let query = transport.query(id).await;
            let result = match query {
                Ok(item) if item.status == ItemStatus::Complete => Ok(item),
                Ok(item) if item.status == ItemStatus::Cancelled => {
                    Err(anyhow::anyhow!("任务已取消"))
                }
                _ => {
                    transport
                        .upload(id, &self.payload_path(id), bytes, &sha256)
                        .await
                }
            };
            match result {
                Ok(completed)
                    if completed.status == ItemStatus::Complete
                        && completed.bytes == Some(bytes)
                        && completed.sha256.as_deref() == Some(&sha256) =>
                {
                    {
                        let mut state = self.state.lock().unwrap();
                        if let Some(record) = state.saved.records.get_mut(&id) {
                            record.entry.status = "complete".into();
                            record.entry.error = None;
                            record.pending_upload = false;
                            record.item = Some(completed);
                        }
                        state.in_flight.remove(&id);
                    }
                    self.transfer(id, "upload", "complete", bytes, Some(bytes), None);
                    let _ = self.persist();
                    let _ = self.events.send(ClientEvent::HistoryChanged);
                    return;
                }
                Ok(_) => last_error = "服务器未确认内容完整提交".into(),
                Err(error) => last_error = format!("{error:#}"),
            }
            {
                let mut state = self.state.lock().unwrap();
                if let Some(record) = state.saved.records.get_mut(&id) {
                    record.retries = attempt.saturating_add(1);
                }
            }
            let _ = self.persist();
        }
        self.fail_record(id, "upload", &last_error);
    }
    async fn preparation_failed(self: Arc<Self>, id: Uuid, error: &str) {
        let transport = {
            let mut state = self.state.lock().unwrap();
            let server_id = state.saved.records.get(&id).and_then(|r| r.server_id);
            if let Some(record) = state.saved.records.get_mut(&id) {
                record.pending_upload = false;
            }
            state
                .transport
                .clone()
                .filter(|t| Some(t.pairing.server_id) == server_id)
        };
        self.fail_record(id, "upload", error);
        if let Some(transport) = transport {
            let _ = transport.cancel(id).await;
        }
    }
    fn fail_record(&self, id: Uuid, direction: &str, error: &str) {
        {
            let mut state = self.state.lock().unwrap();
            if let Some(record) = state.saved.records.get_mut(&id) {
                record.entry.status = "failed".into();
                record.entry.error = Some(error.into());
                if record.retries > 3 {
                    record.pending_upload = false;
                }
            }
            state.in_flight.remove(&id);
            state.pending_local.remove(&id);
        }
        self.transfer(id, direction, "failed", 0, None, Some(error.into()));
        let _ = self.persist();
        let _ = self.events.send(ClientEvent::HistoryChanged);
    }
    pub(super) async fn connection_loop(self: Arc<Self>) {
        while !self.stopped.load(Ordering::SeqCst) {
            let (settings, credential_message) = {
                let state = self.state.lock().unwrap();
                let settings = state
                    .pairing
                    .clone()
                    .map(|p| (state.config.address.clone(), state.config.port, p));
                let message = match &state.credential_state {
                    CredentialState::Loading => {
                        "正在读取系统凭据；如出现系统授权提示，请在提示中处理。本地历史仍可使用。"
                            .to_string()
                    }
                    CredentialState::Ready => "请先设置中转服务器".to_string(),
                    CredentialState::Failed(message) => message.clone(),
                };
                (settings, message)
            };
            let result = if let Some((address, port, pairing)) = settings {
                match transport::Transport::new(&address, port, pairing).await {
                    Ok(transport) => self.clone().run_connection(transport).await,
                    Err(error) => Err(error),
                }
            } else {
                Err(anyhow::anyhow!(credential_message))
            };
            {
                let mut state = self.state.lock().unwrap();
                state.connected = false;
                state.epoch += 1;
            }
            let _ = self.events.send(ClientEvent::Connection {
                connected: false,
                message: result
                    .err()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_else(|| "连接已断开，正在重连".into()),
            });
            tokio::select! { _ = self.reconnect.notified() => (), _ = tokio::time::sleep(Duration::from_secs(2)) => () }
        }
    }
    async fn run_connection(self: Arc<Self>, transport: transport::Transport) -> Result<()> {
        transport.info().await?;
        let mut socket = transport.events().await?;
        let epoch = {
            let mut state = self.state.lock().unwrap();
            if state
                .pairing
                .as_ref()
                .is_none_or(|p| p.server_id != transport.pairing.server_id)
            {
                bail!("服务器设置已改变");
            }
            state.epoch += 1;
            state.connected = false;
            state.transport = Some(transport.clone());
            state.epoch
        };
        let mut snapshotted = false;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                _ = self.reconnect.notified() => { return Ok(()); }
                _ = heartbeat.tick() => {
                    if self.stopped.load(Ordering::SeqCst) { return Ok(()); }
                    socket.send(Message::Ping(vec![].into())).await?;
                }
                message = socket.next() => {
                    let message = message.context("服务器关闭连接")??;
                    let event = match message { Message::Text(text) => serde_json::from_str::<ServerEvent>(&text)?, Message::Close(_) => return Ok(()), _ => continue };
                    match event {
                        ServerEvent::Snapshot { mut items, high_water } => {
                            if snapshotted { bail!("同一连接收到重复快照"); }
                            {
                                let mut state = self.state.lock().unwrap();
                                if state.epoch != epoch || state.pairing.as_ref().is_none_or(|p| p.server_id != transport.pairing.server_id) { bail!("服务器设置已改变"); }
                                state.high_water = high_water; state.latest_sequence = high_water; state.connected = true;
                            }
                            snapshotted = true;
                            items.sort_by_key(|i| i.sequence);
                            for item in items { self.clone().ingest(item, epoch, false, transport.pairing.server_id).await; }
                            let _ = self.events.send(ClientEvent::Connection { connected: true, message: "已连接".into() });
                            let pending: Vec<_> = {
                                let state = self.state.lock().unwrap(); state.saved.records.values().filter(|r| r.prepared && r.pending_upload && r.retries <= 3 && !state.in_flight.contains(&r.entry.id)).map(|r| r.entry.id).collect()
                            };
                            for id in pending { let core = self.clone(); tokio::spawn(async move { core.upload_record(id).await; }); }
                        }
                        ServerEvent::Registered { item } => { if !snapshotted { bail!("服务器未发送原子快照"); } self.clone().ingest(item, epoch, true, transport.pairing.server_id).await; }
                        ServerEvent::Completed { item } => { if !snapshotted { bail!("服务器未发送原子快照"); } self.clone().ingest(item, epoch, true, transport.pairing.server_id).await; }
                        ServerEvent::Progress { id, received_bytes, total_bytes } => {
                            let local = { let state = self.state.lock().unwrap(); state.saved.records.get(&id).is_some_and(|r| r.item.as_ref().is_some_and(|i| i.device_id == state.config.device_id)) };
                            if local { self.transfer(id, "upload", "transferring", received_bytes, Some(total_bytes), None); }
                        }
                        ServerEvent::Failed { item } => {
                            let mut state = self.state.lock().unwrap();
                            if let Some(record) = state.saved.records.get_mut(&item.id) { record.entry.error = item.error.clone(); record.item = Some(item); }
                        }
                        ServerEvent::ResyncRequired => { bail!("事件队列已更新，正在重新获取历史"); }
                    }
                }
            }
        }
    }
    async fn ingest(self: Arc<Self>, item: Item, epoch: u64, live: bool, server_id: Uuid) {
        let (download, generation, download_epoch, can_apply) = {
            let mut state = self.state.lock().unwrap();
            if state.epoch != epoch
                || state
                    .pairing
                    .as_ref()
                    .is_none_or(|p| p.server_id != server_id)
            {
                return;
            }
            state.latest_sequence = state.latest_sequence.max(item.sequence);
            let own = item.device_id == state.config.device_id;
            if !state.saved.records.contains_key(&item.id) {
                state.saved.order += 1;
                let local_order = state.saved.order;
                let receive_generation = state.local_generation;
                state.saved.records.insert(
                    item.id,
                    Record {
                        file_size_limit: None,
                        server_id: Some(server_id),
                        capture_saved: false,
                        download_attempts: 0,
                        receive_generation,
                        receive_epoch: epoch,
                        auto_apply: live,
                        source_refs: vec![],
                        entry: HistoryEntry {
                            available: false,
                            id: item.id,
                            kind: item.kind,
                            device_name: item.device_name.clone(),
                            created_at: item.created_at,
                            sequence: Some(item.sequence),
                            preview: item.preview.clone(),
                            detail: "等待传输".into(),
                            thumbnail: None,
                            status: "pending".into(),
                            error: item.error.clone(),
                        },
                        item: Some(item.clone()),
                        local_order,
                        ordering_sequence: item.sequence,
                        prepared: false,
                        files: vec![],
                        retries: 0,
                        pending_upload: false,
                        unreferenced_at: None,
                    },
                );
            } else if let Some(record) = state.saved.records.get_mut(&item.id) {
                record.entry.sequence = Some(item.sequence);
                record.ordering_sequence = item.sequence;
                // Preserve hashes computed by local preparation before server completion.
                if !own
                    || item.status == ItemStatus::Complete
                    || record.item.as_ref().is_none_or(|i| i.bytes.is_none())
                {
                    record.item = Some(item.clone());
                }
            }
            Self::refresh_history(&mut state);
            let download = item.status == ItemStatus::Complete
                && !state.saved.records[&item.id].prepared
                && state.saved.records[&item.id].download_attempts <= 3
                && !state.in_flight.contains(&item.id);
            if download {
                state.in_flight.insert(item.id);
            }
            (
                download,
                state.saved.records[&item.id].receive_generation,
                state.saved.records[&item.id].receive_epoch,
                state.saved.records[&item.id].auto_apply,
            )
        };
        let _ = self.persist();
        let _ = self.events.send(ClientEvent::HistoryChanged);
        if download {
            let core = self.clone();
            tokio::spawn(async move {
                core.download_record(item, download_epoch, generation, can_apply)
                    .await;
            });
        }
    }
    async fn download_record(self: Arc<Self>, item: Item, epoch: u64, generation: u64, live: bool) {
        let (transport, attempts) = {
            let state = self.state.lock().unwrap();
            let record = &state.saved.records[&item.id];
            (
                state
                    .transport
                    .clone()
                    .filter(|t| record.server_id == Some(t.pairing.server_id)),
                record.download_attempts,
            )
        };
        let Some(transport) = transport else {
            return;
        };
        let id = item.id;
        let payload = self.payload_path(id);
        let temporary = payload.with_extension("downloading");
        self.transfer(id, "download", "start", 0, item.bytes, None);
        let mut error = String::new();
        for attempt in attempts..=3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
            }
            let _ = tokio::fs::remove_file(&temporary).await;
            if let Some(r) = self.state.lock().unwrap().saved.records.get_mut(&id) {
                r.download_attempts = attempt + 1;
            }
            let _ = self.persist();
            let core = self.clone();
            let result = transport
                .download(&item, &temporary, move |bytes| {
                    core.transfer(id, "download", "transferring", bytes, item.bytes, None)
                })
                .await;
            match result {
                Ok(()) => {
                    self.transfer(
                        id,
                        "download",
                        "verifying",
                        item.bytes.unwrap_or(0),
                        item.bytes,
                        None,
                    );
                    let core = self.clone();
                    let item2 = item.clone();
                    let temp2 = temporary.clone();
                    let payload2 = payload.clone();
                    let restored = tokio::task::spawn_blocking(move || {
                        core.finish_download(&item2, &temp2, &payload2)
                    })
                    .await;
                    match restored {
                        Ok(Ok(())) => {
                            // Re-check ALL intent boundaries after full download and restore.
                            let content = self.content(id);
                            if let Ok(content) = content {
                                let _serial = self.capture_serial.lock().unwrap();
                                let mut state = self.state.lock().unwrap();
                                if live
                                    && state.connected
                                    && state.epoch == epoch
                                    && item.sequence > state.high_water
                                    && item.sequence == state.latest_sequence
                                    && generation == state.local_generation
                                    && state.pending_local.is_empty()
                                    && state.pending_file_copy.is_none()
                                {
                                    match self
                                        .clipboard
                                        .write_if_sequence(&content, state.native_sequence)
                                    {
                                        Ok(Some(seq)) => {
                                            state.native_sequence = seq;
                                            state.saved.current = Some(id);
                                        }
                                        Ok(None) => (), // a new native copy always wins
                                        Err(e) => {
                                            if let Some(r) = state.saved.records.get_mut(&id) {
                                                r.entry.error = Some(format!(
                                                    "内容已保存，写入剪贴板失败：{e}"
                                                ));
                                            }
                                        }
                                    }
                                }
                                state.in_flight.remove(&id);
                            }
                            self.transfer(
                                id,
                                "download",
                                "complete",
                                item.bytes.unwrap_or(0),
                                item.bytes,
                                None,
                            );
                            let _ = self.persist();
                            let _ = self.events.send(ClientEvent::HistoryChanged);
                            return;
                        }
                        Ok(Err(e)) => {
                            error = format!("{e:#}");
                            break;
                        }
                        Err(e) => {
                            error = e.to_string();
                            break;
                        }
                    }
                }
                Err(e) => error = format!("{e:#}"),
            }
        }
        let _ = tokio::fs::remove_file(temporary).await;
        self.fail_record(id, "download", &error);
    }
    fn finish_download(&self, item: &Item, temporary: &Path, payload: &Path) -> Result<()> {
        let (files, thumbnail, detail) = match item.kind {
            ContentKind::Text => {
                std::fs::read_to_string(temporary).context("文本不是 UTF-8")?;
                (vec![], None, "文本".into())
            }
            ContentKind::Image => {
                let bytes = std::fs::read(temporary)?;
                let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?;
                (
                    vec![],
                    Self::thumbnail(&bytes),
                    format!("{} × {}", image.width(), image.height()),
                )
            }
            ContentKind::Files => {
                let files = archive::unpack(temporary, &payload.parent().unwrap().join("files"))?;
                let count = files.len();
                (files, None, format!("{count} 个项目"))
            }
        };
        std::fs::rename(temporary, payload)?;
        let mut state = self.state.lock().unwrap();
        if let Some(record) = state.saved.records.get_mut(&item.id) {
            record.prepared = true;
            record.files = files;
            record.entry.thumbnail = thumbnail;
            record.entry.detail = format!("{detail} · {}", human_bytes(item.bytes.unwrap_or(0)));
            record.entry.status = "complete".into();
            record.item = Some(item.clone());
            record.entry.error = None;
        }
        Ok(())
    }
    pub(super) fn cleanup(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let mut protected: HashSet<_> = state.saved.history.iter().copied().collect();
        protected.extend(state.saved.current);
        protected.extend(&state.in_flight);
        protected.extend(self.saving.lock().unwrap().keys().copied());
        for active in state
            .in_flight
            .iter()
            .filter_map(|id| state.saved.records.get(id))
        {
            for (id, candidate) in &state.saved.records {
                if candidate.files.iter().any(|p| {
                    active
                        .source_refs
                        .iter()
                        .any(|r| r.starts_with(p) || p.starts_with(r))
                }) {
                    protected.insert(*id);
                }
            }
        }
        // On restart protect every native file reference before considering GC.
        if let Ok(Some(ClipboardContent::Files(paths))) = self.clipboard.read() {
            for (id, record) in &state.saved.records {
                if record
                    .files
                    .iter()
                    .any(|f| paths.iter().any(|p| p.starts_with(f) || f.starts_with(p)))
                {
                    protected.insert(*id);
                }
            }
        }
        let mut remove = Vec::new();
        for (id, record) in &mut state.saved.records {
            if protected.contains(id) || record.pending_upload {
                record.unreferenced_at = None;
            } else {
                let since = record.unreferenced_at.get_or_insert(now);
                if now - *since >= 24 * 60 * 60 * 1000 {
                    remove.push(*id);
                }
            }
        }
        for id in remove {
            let path = self.payload_path(id).parent().unwrap().to_path_buf();
            if path.exists() {
                std::fs::remove_dir_all(path)?;
            }
            state.saved.records.remove(&id);
        }
        drop(state);
        self.persist()
    }
}
fn human_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
