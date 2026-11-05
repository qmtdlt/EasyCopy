use super::*;

/// Decimal MB, matching the size shown in the confirmation dialog.
pub const FILE_CONFIRM_BYTES: u64 = 50_000_000;
pub(super) fn default_threshold_mb() -> u64 {
    FILE_CONFIRM_BYTES / 1_000_000
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUploadPrompt {
    pub id: Uuid,
    pub bytes: u64,
    pub threshold_mb: u64,
}

pub(super) struct PendingFileCopy {
    id: Uuid,
    paths: Vec<PathBuf>,
    generation: u64,
    sequence: u64,
    bytes: Option<u64>,
    limit: u64,
}

impl ClientCore {
    pub fn file_upload_threshold_mb(&self) -> u64 {
        self.state.lock().unwrap().config.file_upload_threshold_mb
    }

    pub fn set_file_upload_threshold_mb(&self, mb: u64) -> Result<()> {
        if mb == 0 || mb.checked_mul(1_000_000).is_none() {
            bail!("请输入有效的正整数，单位为 MB");
        }
        let mut state = self.state.lock().unwrap();
        let previous = state.config.file_upload_threshold_mb;
        state.config.file_upload_threshold_mb = mb;
        if let Err(error) = Self::atomic_json(&self.directory.join("settings.json"), &state.config)
        {
            state.config.file_upload_threshold_mb = previous;
            return Err(error);
        }
        Ok(())
    }
    pub fn file_upload_prompt(&self) -> Option<FileUploadPrompt> {
        self.state
            .lock()
            .unwrap()
            .pending_file_copy
            .as_ref()
            .and_then(|copy| {
                copy.bytes.map(|bytes| FileUploadPrompt {
                    id: copy.id,
                    bytes,
                    threshold_mb: copy.limit / 1_000_000,
                })
            })
    }

    // Called under capture_serial. No rejected/pending request is persisted or
    // registered with the relay, so neither restart nor reconnect can send it.
    pub(super) fn invalidate_file_prompt(self: &Arc<Self>) {
        self.cancel_older_transfers();
        let mut state = self.state.lock().unwrap();
        state.local_generation += 1;
        state.saved.current = None;
        if state.pending_file_copy.take().is_some() {
            let _ = self.events.send(ClientEvent::FileUploadPrompt(None));
        }
    }

    pub(super) fn accept_local_content(
        self: &Arc<Self>,
        content: ClipboardContent,
        sequence: u64,
    ) -> Result<Uuid> {
        self.invalidate_file_prompt();
        let id = Uuid::new_v4();
        if let ClipboardContent::Files(paths) = content {
            {
                let mut state = self.state.lock().unwrap();
                state.pending_file_copy = Some(PendingFileCopy {
                    id,
                    paths: paths.clone(),
                    generation: state.local_generation,
                    sequence,
                    bytes: None,
                    limit: state
                        .config
                        .file_upload_threshold_mb
                        .checked_mul(1_000_000)
                        .filter(|n| *n > 0)
                        .unwrap_or(FILE_CONFIRM_BYTES),
                });
            }
            let core = self.clone();
            tokio::spawn(async move {
                let measured = tokio::task::spawn_blocking(move || total_file_bytes(&paths)).await;
                let result = measured.map_err(anyhow::Error::from).and_then(|r| r);
                if let Err(error) = core.finish_file_measurement(id, result) {
                    core.transfer(
                        id,
                        "upload",
                        "failed",
                        0,
                        None,
                        Some(format!("无法同步文件：{error:#}")),
                    );
                }
            });
        } else {
            self.enqueue_content(id, content, None)?;
        }
        Ok(id)
    }

    fn finish_file_measurement(self: &Arc<Self>, id: Uuid, result: Result<u64>) -> Result<()> {
        let _serial = self.capture_serial.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        if !state.pending_file_copy.as_ref().is_some_and(|p| p.id == id) {
            return Ok(());
        }
        let mut copy = state.pending_file_copy.take().unwrap();
        if self.stopped.load(Ordering::SeqCst)
            || copy.generation != state.local_generation
            || self.clipboard.sequence()? != copy.sequence
        {
            return Ok(());
        }
        let bytes = result?;
        let limit = copy.limit;
        if bytes > limit {
            copy.bytes = Some(bytes);
            state.pending_file_copy = Some(copy);
            let _ = self
                .events
                .send(ClientEvent::FileUploadPrompt(Some(FileUploadPrompt {
                    id,
                    bytes,
                    threshold_mb: limit / 1_000_000,
                })));
        } else {
            drop(state);
            self.enqueue_content(id, ClipboardContent::Files(copy.paths), Some(limit))?;
        }
        Ok(())
    }

    pub fn resolve_file_upload(self: &Arc<Self>, id: Uuid, upload: bool) -> Result<()> {
        let _serial = self.capture_serial.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        if !state
            .pending_file_copy
            .as_ref()
            .is_some_and(|p| p.id == id && p.bytes.is_some())
        {
            return Ok(()); // repeated/stale answers cannot authorize another copy
        }
        let copy = state.pending_file_copy.take().unwrap();
        let _ = self.events.send(ClientEvent::FileUploadPrompt(None));
        let current = copy.generation == state.local_generation
            && self.clipboard.sequence()? == copy.sequence
            && !self.stopped.load(Ordering::SeqCst);
        state.local_generation += 1;
        drop(state);
        if upload && current {
            self.enqueue_content(id, ClipboardContent::Files(copy.paths), None)?;
        }
        Ok(())
    }

    // capture_serial is held by every caller; registering and caching start only here.
    fn enqueue_content(
        &self,
        id: Uuid,
        content: ClipboardContent,
        limit: Option<u64>,
    ) -> Result<()> {
        let transport = {
            let mut state = self.state.lock().unwrap();
            self.capture_locked(&mut state, &content, id);
            state.saved.records.get_mut(&id).unwrap().file_size_limit = limit;
            if state.connected {
                state.transport.clone()
            } else {
                None
            }
        };
        if let Err(error) = self.save_capture(id, &content) {
            let mut state = self.state.lock().unwrap();
            state.pending_local.remove(&id);
            state.in_flight.remove(&id);
            if let Some(r) = state.saved.records.get_mut(&id) {
                r.entry.status = "failed".into();
                r.entry.error = Some(error.to_string());
            }
            drop(state);
            let _ = self.persist();
            let _ = self.events.send(ClientEvent::HistoryChanged);
            return Err(error);
        }
        self.local_tx.send(LocalCopy {
            id,
            content,
            transport,
        })?;
        self.persist()?;
        let _ = self.events.send(ClientEvent::HistoryChanged);
        Ok(())
    }
}

fn total_file_bytes(paths: &[PathBuf]) -> Result<u64> {
    let mut total = 0u64;
    for root in paths {
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .context("文件总大小超出支持范围")?;
            } else if !metadata.is_dir() {
                bail!("不支持符号链接或特殊文件：{}", entry.path().display());
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sized(path: &Path, bytes: u64) {
        std::fs::File::create(path).unwrap().set_len(bytes).unwrap();
    }
    async fn until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    fn setup() -> (tempfile::TempDir, Arc<ClientCore>, Arc<MemoryClipboard>) {
        let root = tempfile::tempdir().unwrap();
        let clip = Arc::new(MemoryClipboard::default());
        let core =
            ClientCore::open_with_clipboard(root.path().join("profile"), clip.clone()).unwrap();
        (root, core, clip)
    }

    #[tokio::test]
    async fn custom_threshold_applies_to_next_copy_and_survives_restart() {
        let (root, core, clip) = setup();
        core.set_file_upload_threshold_mb(10).unwrap();
        let path = root.path().join("custom-limit");
        sized(&path, 10_000_001);
        let content = ClipboardContent::Files(vec![path]);
        let first = core.submit_content(content.clone()).await.unwrap();
        until(|| core.file_upload_prompt().is_some()).await;
        assert_eq!(core.file_upload_prompt().unwrap().threshold_mb, 10);
        core.set_file_upload_threshold_mb(100).unwrap();
        assert_eq!(core.file_upload_prompt().unwrap().threshold_mb, 10);
        core.resolve_file_upload(first, false).unwrap();
        let second = core.submit_content(content).await.unwrap();
        until(|| !core.history().is_empty()).await;
        assert!(core.file_upload_prompt().is_none());
        assert_eq!(
            core.state.lock().unwrap().saved.records[&second].file_size_limit,
            Some(100_000_000)
        );
        core.set_file_upload_threshold_mb(37).unwrap();
        assert!(core.set_file_upload_threshold_mb(0).is_err());
        assert!(core.set_file_upload_threshold_mb(u64::MAX).is_err());
        let reopened = ClientCore::open_with_clipboard(root.path().join("profile"), clip).unwrap();
        assert_eq!(reopened.file_upload_threshold_mb(), 37);
    }

    #[test]
    fn older_settings_default_to_50_mb_without_losing_server_configuration() {
        let (root, core, clip) = setup();
        let settings = root.path().join("profile/settings.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("file_upload_threshold_mb");
        json["address"] = "example.local".into();
        std::fs::write(&settings, serde_json::to_vec(&json).unwrap()).unwrap();
        drop(core);
        let reopened = ClientCore::open_with_clipboard(root.path().join("profile"), clip).unwrap();
        assert_eq!(reopened.file_upload_threshold_mb(), 50);
        assert_eq!(reopened.settings().address, "example.local");
    }

    #[tokio::test]
    async fn aggregate_threshold_includes_nested_files_and_allows_exactly_50_mb() {
        let (root, core, _) = setup();
        let folder = root.path().join("folder");
        std::fs::create_dir_all(folder.join("nested/empty")).unwrap();
        sized(&folder.join("nested/a"), 25_000_000);
        let other = root.path().join("other");
        sized(&other, 25_000_000);
        let paths = vec![folder, other.clone()];
        assert_eq!(total_file_bytes(&paths).unwrap(), FILE_CONFIRM_BYTES);
        let id = core
            .submit_content(ClipboardContent::Files(paths.clone()))
            .await
            .unwrap();
        until(|| core.history().iter().any(|e| e.id == id)).await;
        assert!(core.file_upload_prompt().is_none());
        assert_eq!(
            core.state.lock().unwrap().saved.records[&id].file_size_limit,
            Some(FILE_CONFIRM_BYTES)
        );
        sized(&other, 25_000_001);
        let next = core
            .submit_content(ClipboardContent::Files(paths))
            .await
            .unwrap();
        until(|| core.file_upload_prompt().is_some()).await;
        assert_eq!(
            core.file_upload_prompt().unwrap().bytes,
            FILE_CONFIRM_BYTES + 1
        );
        assert!(!core.history().iter().any(|e| e.id == next));
    }

    #[tokio::test]
    async fn decline_keeps_clipboard_and_does_not_reprompt_until_a_new_native_copy() {
        let (root, core, clip) = setup();
        let path = root.path().join("big");
        sized(&path, FILE_CONFIRM_BYTES + 1);
        let content = ClipboardContent::Files(vec![path]);
        let monitor = tokio::spawn(core.clone().monitor_loop());
        clip.set(content.clone());
        until(|| core.file_upload_prompt().is_some()).await;
        let first = core.file_upload_prompt().unwrap().id;
        let sequence = clip.sequence().unwrap();
        core.resolve_file_upload(first, false).unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(core.file_upload_prompt().is_none());
        assert!(core.history().is_empty());
        assert!(core
            .local_rx
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .try_recv()
            .is_err());
        assert_eq!(clip.content(), Some(content.clone()));
        assert_eq!(clip.sequence().unwrap(), sequence);
        clip.set(content);
        until(|| core.file_upload_prompt().is_some()).await;
        assert_ne!(core.file_upload_prompt().unwrap().id, first);
        core.shutdown();
        monitor.await.unwrap();
        let reopened = ClientCore::open_with_clipboard(root.path().join("profile"), clip).unwrap();
        reopened.clone().recover_preparations().await;
        assert!(reopened.history().is_empty());
        assert!(reopened.file_upload_prompt().is_none());
        assert!(std::fs::read_dir(root.path().join("profile/cache"))
            .unwrap()
            .next()
            .is_none());
    }

    #[tokio::test]
    async fn yes_enqueues_once_without_rewriting_clipboard() {
        let (root, core, clip) = setup();
        let path = root.path().join("big");
        sized(&path, FILE_CONFIRM_BYTES + 1);
        let id = core
            .submit_content(ClipboardContent::Files(vec![path]))
            .await
            .unwrap();
        until(|| core.file_upload_prompt().is_some()).await;
        let sequence = clip.sequence().unwrap();
        core.resolve_file_upload(id, true).unwrap();
        core.resolve_file_upload(id, true).unwrap();
        assert_eq!(core.history().len(), 1);
        assert_eq!(core.history()[0].id, id);
        assert_eq!(clip.sequence().unwrap(), sequence);
        let mut rx = core.local_rx.lock().unwrap();
        assert_eq!(rx.as_mut().unwrap().try_recv().unwrap().id, id);
        assert!(rx.as_mut().unwrap().try_recv().is_err());
    }

    #[tokio::test]
    async fn newer_copy_cancels_prompt_and_stale_yes_cannot_send_files() {
        let (root, core, clip) = setup();
        let path = root.path().join("big");
        sized(&path, FILE_CONFIRM_BYTES + 1);
        let id = core
            .submit_content(ClipboardContent::Files(vec![path]))
            .await
            .unwrap();
        until(|| core.file_upload_prompt().is_some()).await;
        let text = ClipboardContent::Text("new text".into());
        let newer = core.submit_content(text.clone()).await.unwrap();
        core.resolve_file_upload(id, true).unwrap();
        assert!(core.file_upload_prompt().is_none());
        assert_eq!(core.history().len(), 1);
        assert_eq!(core.history()[0].id, newer);
        assert_eq!(clip.content(), Some(text));
    }

    #[tokio::test]
    async fn native_copy_before_monitor_tick_invalidates_approval() {
        let (root, core, clip) = setup();
        let path = root.path().join("big");
        sized(&path, FILE_CONFIRM_BYTES + 1);
        let id = core
            .submit_content(ClipboardContent::Files(vec![path]))
            .await
            .unwrap();
        until(|| core.file_upload_prompt().is_some()).await;
        clip.set(ClipboardContent::Text("not yet polled".into()));
        core.resolve_file_upload(id, true).unwrap();
        assert!(core.history().is_empty());
    }

    #[tokio::test]
    async fn file_history_cannot_copy_but_save_still_works() {
        let (root, core, clip) = setup();
        let path = root.path().join("small");
        std::fs::write(&path, "saved content").unwrap();
        let content = ClipboardContent::Files(vec![path]);
        let id = core.submit_content(content.clone()).await.unwrap();
        until(|| !core.history().is_empty()).await;
        let worker = tokio::spawn(core.clone().local_loop());
        until(|| core.history()[0].available).await;
        let sequence = clip.sequence().unwrap();
        assert!(core
            .copy_item(id)
            .await
            .unwrap_err()
            .to_string()
            .contains("仅支持另存为"));
        assert_eq!(clip.sequence().unwrap(), sequence);
        assert_eq!(core.history().len(), 1);
        let saved = root.path().join("saved");
        std::fs::create_dir(&saved).unwrap();
        core.save_item(id, saved.clone()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(saved.join("small")).unwrap(),
            "saved content"
        );
        worker.abort();
    }

    #[test]
    fn file_growth_cannot_bypass_confirmation_during_pack() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("growing");
        sized(&path, FILE_CONFIRM_BYTES);
        assert_eq!(
            total_file_bytes(&[path.clone()]).unwrap(),
            FILE_CONFIRM_BYTES
        );
        sized(&path, FILE_CONFIRM_BYTES + 1);
        let target = root.path().join("archive");
        assert!(archive::pack_with_limit(&[path], &target, Some(FILE_CONFIRM_BYTES)).is_err());
        assert!(!target.exists());
    }
}
