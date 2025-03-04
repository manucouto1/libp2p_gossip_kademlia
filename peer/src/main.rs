use std::error::Error;

use clap::Parser;
use dotenv::dotenv;
use peer::Peer;
use std::env;
use tokio::{io, io::AsyncBufReadExt, select};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[clap(name = "libp2p DCUtR client")]
struct Opts {
    /// Fixed value to generate deterministic peer id.
    #[clap(long)]
    secret_key_seed: u8,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let relay_host = env::var("RELAY_HOST").expect("RELAY_HOST no está definido en .env");
    let relay_port = env::var("RELAY_PORT").expect("RELAY_PORT no está definido en .env");
    let relay_peer_id = env::var("RELAY_PEER_ID").expect("RELAY_PEER_ID no está definido en .env");

    let relay_address = format!(
        "/ip4/{}/tcp/{}/p2p/{}",
        relay_host, relay_port, relay_peer_id
    );
    let opts = Opts::parse();

    let mut stdin = io::BufReader::new(io::stdin()).lines();
    let mut peer = Peer::new(opts.secret_key_seed, relay_address.parse().unwrap())
        .await
        .unwrap();

    peer.run("chat_room".to_string());

    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                peer.send_message(line.to_string()).await;
            }
        }
    }
}
