use crate::archive::ensure_space;
use anyhow::{bail, Context, Result};
use easycopy_protocol::*;
use futures_util::StreamExt;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, net::TcpStream};
use tokio_tungstenite::{
    tungstenite::client::IntoClientRequest, Connector, MaybeTlsStream, WebSocketStream,
};

const TRANSFER_BUFFER_BYTES: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

// Keep the reservation until Tokio's pending file write has completed. flush
// waits for that write; durability is still provided by sync_all at completion.
async fn write_download_block(
    file: &mut tokio::fs::File,
    directory: &Path,
    block: &[u8],
) -> Result<()> {
    if !block.is_empty() {
        let _reservation = crate::archive::reserve_write(directory, block.len() as u64)?;
        file.write_all(block).await?;
        file.flush().await?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct Transport {
    pub pairing: Pairing,
    client: Client,
    address: SocketAddr,
    port: u16,
    tls: Arc<rustls::ClientConfig>,
}
impl Transport {
    pub async fn new(address: &str, port: u16, pairing: Pairing) -> Result<Self> {
        pairing.validate()?;
        if address.trim().is_empty() || address.contains(['/', '@', '?', '#']) || port == 0 {
            bail!("请输入服务器 IP 或主机名及有效端口");
        }
        let address = tokio::net::lookup_host((address.trim(), port))
            .await?
            .next()
            .context("服务器地址无法解析")?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let certs: Vec<_> = rustls_pemfile::certs(&mut pairing.ca_pem.as_bytes())
            .collect::<std::result::Result<_, _>>()?;
        if certs.is_empty() {
            bail!("配对根证书为空");
        }
        let mut roots = rustls::RootCertStore::empty();
        for cert in certs {
            roots.add(cert)?;
        }
        let tls = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(pairing.ca_pem.as_bytes())?)
            .resolve(&pairing.logical_name, address)
            .connect_timeout(Duration::from_secs(8))
            .build()?;
        Ok(Self {
            pairing,
            client,
            address,
            port,
            tls,
        })
    }
    fn url(&self, path: &str) -> String {
        format!("https://{}:{}{path}", self.pairing.logical_name, self.port)
    }
    async fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ApiError>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| format!("HTTP {status}"));
        bail!("{message}")
    }
    pub async fn info(&self) -> Result<ServerInfo> {
        let info: ServerInfo = Self::checked(
            self.client
                .get(self.url("/v1/info"))
                .bearer_auth(&self.pairing.secret)
                .timeout(Duration::from_secs(12))
                .send()
                .await?,
        )
        .await?
        .json()
        .await?;
        if info.protocol_version != PROTOCOL_VERSION
            || info.server_id != self.pairing.server_id
            || info.logical_name != self.pairing.logical_name
        {
            bail!("服务器身份或协议不匹配");
        }
        Ok(info)
    }
    pub async fn register(&self, input: &RegisterItem) -> Result<Item> {
        Ok(Self::checked(
            self.client
                .post(self.url("/v1/items"))
                .bearer_auth(&self.pairing.secret)
                .json(input)
                .timeout(Duration::from_secs(12))
                .send()
                .await?,
        )
        .await?
        .json()
        .await?)
    }
    pub async fn query(&self, id: uuid::Uuid) -> Result<Item> {
        Ok(Self::checked(
            self.client
                .get(self.url(&format!("/v1/items/{id}")))
                .bearer_auth(&self.pairing.secret)
                .timeout(Duration::from_secs(12))
                .send()
                .await?,
        )
        .await?
        .json()
        .await?)
    }
    pub async fn upload(
        &self,
        id: uuid::Uuid,
        path: &Path,
        bytes: u64,
        sha256: &str,
    ) -> Result<Item> {
        let stream = tokio_util::io::ReaderStream::with_capacity(
            tokio::fs::File::open(path).await?,
            TRANSFER_BUFFER_BYTES,
        );
        let response = self
            .client
            .put(self.url(&format!("/v1/items/{id}/payload")))
            .bearer_auth(&self.pairing.secret)
            .header(PAYLOAD_HASH_HEADER, sha256)
            .header(PAYLOAD_SIZE_HEADER, bytes.to_string())
            .header(reqwest::header::CONTENT_LENGTH, bytes)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await?;
        Ok(Self::checked(response).await?.json().await?)
    }
    pub async fn download(
        &self,
        item: &Item,
        destination: &Path,
        progress: impl Fn(u64) + Send,
    ) -> Result<()> {
        let expected = item.bytes.context("服务器缺少内容长度")?;
        let digest = item.sha256.as_ref().context("服务器缺少内容摘要")?;
        let directory = destination.parent().context("缓存目录无效")?;
        tokio::fs::create_dir_all(directory).await?;
        ensure_space(directory, expected)?;
        let response = Self::checked(
            self.client
                .get(self.url(&format!("/v1/items/{}/payload", item.id)))
                .bearer_auth(&self.pairing.secret)
                .send()
                .await?,
        )
        .await?;
        if let Some(length) = response.content_length() {
            if length != expected {
                bail!("下载长度与登记内容不一致");
            }
        }
        let mut output = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .await?;
        let mut stream = response.bytes_stream();
        let mut received = 0u64;
        let mut sha = Sha256::new();
        let mut pending = Vec::with_capacity(TRANSFER_BUFFER_BYTES);
        let mut last_progress = Instant::now();
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(45), stream.next())
            .await
            .context("下载长时间未收到数据")?
        {
            let chunk = chunk?;
            if received.saturating_add(chunk.len() as u64) > expected {
                bail!("服务器发送超出声明长度的内容");
            }
            let mut remaining = chunk.as_ref();
            while !remaining.is_empty() {
                let take = remaining.len().min(TRANSFER_BUFFER_BYTES - pending.len());
                pending.extend_from_slice(&remaining[..take]);
                remaining = &remaining[take..];
                if pending.len() == TRANSFER_BUFFER_BYTES {
                    write_download_block(&mut output, directory, &pending).await?;
                    pending.clear();
                }
            }
            sha.update(&chunk);
            received += chunk.len() as u64;
            if received < expected && last_progress.elapsed() >= PROGRESS_INTERVAL {
                progress(received);
                last_progress = Instant::now();
            }
        }
        write_download_block(&mut output, directory, &pending).await?;
        output.sync_all().await?;
        if received != expected || hex::encode(sha.finalize()) != *digest {
            bail!("下载内容校验失败");
        }
        progress(received);
        Ok(())
    }
    pub async fn cancel(&self, id: uuid::Uuid) -> Result<()> {
        Self::checked(
            self.client
                .delete(self.url(&format!("/v1/items/{id}")))
                .bearer_auth(&self.pairing.secret)
                .timeout(Duration::from_secs(12))
                .send()
                .await?,
        )
        .await?;
        Ok(())
    }
    pub async fn events(&self) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        let mut request = format!(
            "wss://{}:{}/v1/events",
            self.pairing.logical_name, self.port
        )
        .into_client_request()?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.pairing.secret).parse()?,
        );
        let socket = tokio::time::timeout(Duration::from_secs(8), TcpStream::connect(self.address))
            .await??;
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(12),
            tokio_tungstenite::client_async_tls_with_config(
                request,
                socket,
                None,
                Some(Connector::Rustls(self.tls.clone())),
            ),
        )
        .await??;
        Ok(ws)
    }
}
