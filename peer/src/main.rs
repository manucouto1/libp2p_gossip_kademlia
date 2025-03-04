use std::error::Error;

use clap::Parser;
use peer::Peer;
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
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opts = Opts::parse();

    let mut stdin = io::BufReader::new(io::stdin()).lines();
    let mut peer = Peer::new(
        opts.secret_key_seed,
        "/ip4/172.20.0.100/tcp/4000/p2p/12D3KooWR2KSRQWyanR1dPvnZkXt296xgf3FFn8135szya3zYYwY"
            .parse()
            .unwrap(),
    )
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
