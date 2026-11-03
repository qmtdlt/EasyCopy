use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;
pub const HISTORY_LIMIT: usize = 10;
pub const DISK_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;
pub const PAIRING_PREFIX: &str = "easycopy1:";
pub const PAYLOAD_HASH_HEADER: &str = "x-easycopy-sha256";
pub const PAYLOAD_SIZE_HEADER: &str = "x-easycopy-size";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Image,
    Files,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Registered,
    Uploading,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterItem {
    pub id: Uuid,
    pub device_id: Uuid,
    pub device_name: String,
    pub kind: ContentKind,
    pub created_at: i64,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub id: Uuid,
    pub device_id: Uuid,
    pub device_name: String,
    pub kind: ContentKind,
    pub created_at: i64,
    pub preview: String,
    pub sequence: u64,
    pub status: ItemStatus,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub protocol_version: u32,
    pub server_id: Uuid,
    pub logical_name: String,
}

// Snapshot and live subscription are acquired under the server's one state lock.
// Only records with sequence > snapshot.high_water may affect native clipboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Snapshot {
        items: Vec<Item>,
        high_water: u64,
    },
    Registered {
        item: Item,
    },
    Progress {
        id: Uuid,
        received_bytes: u64,
        total_bytes: u64,
    },
    Completed {
        item: Item,
    },
    Failed {
        item: Item,
    },
    ResyncRequired,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub version: u32,
    pub server_id: Uuid,
    pub logical_name: String,
    pub ca_pem: String,
    pub secret: String,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("server_id", &self.server_id)
            .finish_non_exhaustive()
    }
}

impl Pairing {
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        Ok(format!(
            "{PAIRING_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)
        ))
    }
    pub fn decode(value: &str) -> Result<Self> {
        if value.len() > 32768 {
            bail!("连接密钥过长");
        }
        let encoded = value
            .trim()
            .strip_prefix(PAIRING_PREFIX)
            .context("连接密钥格式无效")?;
        let pairing: Self = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(encoded)
                .context("连接密钥编码无效")?,
        )
        .context("连接密钥内容无效")?;
        pairing.validate()?;
        Ok(pairing)
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != PROTOCOL_VERSION {
            bail!("连接密钥协议版本不支持");
        }
        if self.logical_name != format!("relay.{}.invalid", self.server_id) {
            bail!("服务器逻辑名称无效");
        }
        if self.secret.len() < 32
            || !self
                .secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("认证密钥无效");
        }
        if !self.ca_pem.starts_with("-----BEGIN CERTIFICATE-----")
            || self.ca_pem.contains("PRIVATE KEY")
        {
            bail!("根证书无效");
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pairing_rejects_identity_substitution() {
        let p = Pairing {
            version: 1,
            server_id: Uuid::new_v4(),
            logical_name: "evil.example".into(),
            ca_pem: "-----BEGIN CERTIFICATE-----".into(),
            secret: "a".repeat(64),
        };
        assert!(p.encode().is_err());
        assert!(!format!("{p:?}").contains(&p.secret));
    }
}
