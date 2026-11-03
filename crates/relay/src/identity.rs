use anyhow::{Context, Result};
use chrono::Datelike;
use easycopy_protocol::{Pairing, ServerInfo, PROTOCOL_VERSION};
use rand::RngCore;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
struct StoredIdentity {
    server_id: Uuid,
    secret: String,
}

#[derive(Clone)]
pub struct Identity {
    pub pairing: Pairing,
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

pub fn secure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

impl Identity {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        secure_directory(data_dir)?;
        let cert_dir = data_dir.join("identity");
        secure_directory(&cert_dir)?;
        // Serialize first boot even when a pairing command races with the daemon.
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(cert_dir.join("init.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let manifest = cert_dir.join("identity.json");
        if !manifest.exists() {
            let server_id = Uuid::new_v4();
            let name = format!("relay.{server_id}.invalid");
            let mut secret_bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut secret_bytes);
            let identity = StoredIdentity {
                server_id,
                secret: hex::encode(secret_bytes),
            };
            let year = chrono::Utc::now().year();
            let ca_key = KeyPair::generate()?;
            let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
            ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
            ca_params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
            ca_params.not_after = rcgen::date_time_ymd(year + 20, 1, 1);
            ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            ca_params.distinguished_name = DistinguishedName::new();
            ca_params.distinguished_name.push(
                DnType::CommonName,
                format!("EasyCopy private root {server_id}"),
            );
            let ca = ca_params.self_signed(&ca_key)?;
            let server_key = KeyPair::generate()?;
            let mut server_params = CertificateParams::new(vec![name.clone()])?;
            server_params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
            server_params.not_after = rcgen::date_time_ymd(year + 10, 1, 1);
            server_params
                .distinguished_name
                .push(DnType::CommonName, name);
            server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let cert = server_params.signed_by(&server_key, &ca, &ca_key)?;
            // The manifest is the commit record: it is published only after every key/cert exists.
            private_write(
                &cert_dir.join("ca-key.pem"),
                ca_key.serialize_pem().as_bytes(),
            )?;
            private_write(&cert_dir.join("ca.pem"), ca.pem().as_bytes())?;
            private_write(
                &cert_dir.join("server-key.pem"),
                server_key.serialize_pem().as_bytes(),
            )?;
            private_write(&cert_dir.join("server.pem"), cert.pem().as_bytes())?;
            private_write(&manifest, &serde_json::to_vec(&identity)?)?;
        }
        let identity: StoredIdentity = serde_json::from_slice(&fs::read(&manifest)?)
            .context("服务器身份文件损坏，请从可信备份恢复，不能自动重新配对")?;
        let ca_pem = fs::read_to_string(cert_dir.join("ca.pem"))?;
        let certificate_path = cert_dir.join("server.pem");
        let private_key_path = cert_dir.join("server-key.pem");
        for path in [
            &certificate_path,
            &private_key_path,
            &cert_dir.join("ca-key.pem"),
        ] {
            anyhow::ensure!(path.is_file(), "服务器身份文件缺失，请恢复身份目录");
        }
        let pairing = Pairing {
            version: PROTOCOL_VERSION,
            server_id: identity.server_id,
            logical_name: format!("relay.{}.invalid", identity.server_id),
            ca_pem,
            secret: identity.secret,
        };
        pairing.validate()?;
        Ok(Self {
            pairing,
            certificate_path,
            private_key_path,
        })
    }

    pub fn info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: PROTOCOL_VERSION,
            server_id: self.pairing.server_id,
            logical_name: self.pairing.logical_name.clone(),
        }
    }
}
