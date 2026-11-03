use anyhow::{Context, Result};
use easycopy_relay::{identity::Identity, serve};
use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "easycopy_relay=info".into()),
        )
        .init();
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "serve".into());
    let mut data_dir =
        PathBuf::from(std::env::var_os("EASYCOPY_DATA").unwrap_or_else(|| "data".into()));
    let mut bind: SocketAddr = std::env::var("EASYCOPY_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8443".into())
        .parse()
        .context("EASYCOPY_BIND 格式无效")?;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = PathBuf::from(args.next().context("缺少 --data-dir 值")?),
            "--bind" => bind = args.next().context("缺少 --bind 值")?.parse()?,
            _ => anyhow::bail!("未知参数：{arg}"),
        }
    }
    match command.as_str() {
        "serve" => serve(data_dir, bind).await,
        "pairing" => {
            // This explicit trusted-terminal operation is the only place credentials are printed.
            println!("{}", Identity::load_or_create(&data_dir)?.pairing.encode()?);
            Ok(())
        }
        "healthcheck" => {
            let identity = Identity::load_or_create(&data_dir)?;
            let pairing = identity.pairing;
            let address = SocketAddr::new(
                if bind.ip().is_unspecified() {
                    "127.0.0.1".parse()?
                } else {
                    bind.ip()
                },
                bind.port(),
            );
            let client = reqwest::Client::builder()
                .use_rustls_tls()
                .tls_built_in_root_certs(false)
                .add_root_certificate(reqwest::Certificate::from_pem(pairing.ca_pem.as_bytes())?)
                .resolve(&pairing.logical_name, address)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()?;
            client
                .get(format!(
                    "https://{}:{}/v1/info",
                    pairing.logical_name,
                    bind.port()
                ))
                .bearer_auth(&pairing.secret)
                .send()
                .await?
                .error_for_status()?;
            Ok(())
        }
        _ => anyhow::bail!(
            "用法：easycopy-relay [serve|pairing|healthcheck] [--data-dir 目录] [--bind IP:端口]"
        ),
    }
}
