//! Read-only connection probe using the same transport as the desktop client.
//! Read the pairing string from stdin so it never appears in process arguments.
use anyhow::{Context, Result};
use easycopy_client_core::transport::Transport;
use easycopy_protocol::Pairing;
use std::io::Read;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let address = args.next().context("server address required")?;
    let port: u16 = args.next().context("port required")?.parse()?;
    let mut encoded = String::new();
    std::io::stdin().read_to_string(&mut encoded)?;
    let transport = Transport::new(&address, port, Pairing::decode(&encoded)?).await?;
    let info = transport.info().await?;
    println!("HTTPS identity verified: {}", info.server_id);
    let mut events = transport.events().await?;
    events.close(None).await?;
    println!("WSS connection verified");
    Ok(())
}
