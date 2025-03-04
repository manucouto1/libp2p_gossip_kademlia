use std::{error::Error, net::Ipv4Addr};

use clap::Parser;
use futures::StreamExt;
use libp2p::{
    core::{Multiaddr, multiaddr::Protocol},
    identify, identity,
    kad::{self, Mode, store::MemoryStore},
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tracing_subscriber::EnvFilter;

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay: relay::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}

#[derive(Debug, Parser)]
#[clap(name = "libp2p relay")]
struct Opt {
    /// Fixed value to generate deterministic peer id
    #[clap(long)]
    secret_key_seed: u8,

    /// The port used to listen on all interfaces
    #[clap(long)]
    port: u16,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

    // Create a static known PeerId based on given secret
    let local_key: identity::Keypair = generate_ed25519(opt.secret_key_seed);

    tracing::info!("Local peer ID: {:?}", local_key.public().to_peer_id());

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| Behaviour {
            relay: relay::Behaviour::new(key.public().to_peer_id(), relay::Config::default()),
            ping: ping::Behaviour::new(ping::Config::new()),
            identify: identify::Behaviour::new(identify::Config::new(
                "/TODO/0.0.1".to_string(),
                key.public(),
            )),
            kademlia: kad::Behaviour::new(
                key.public().to_peer_id(),
                MemoryStore::new(key.public().to_peer_id()),
            ),
        })?
        .build();

    swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

    // También en TCP y QUIC
    let listen_addr_tcp = Multiaddr::empty()
        .with(Protocol::Ip4(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Tcp(opt.port));
    swarm.listen_on(listen_addr_tcp)?;

    let listen_addr_quic = Multiaddr::empty()
        .with(Protocol::Ip4(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Udp(opt.port))
        .with(Protocol::QuicV1);
    swarm.listen_on(listen_addr_quic)?;

    loop {
        match swarm.next().await.expect("Infinite Stream.") {
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info: identify::Info { listen_addrs, .. },
                ..
            })) => {
                if peer_id != swarm.local_peer_id().clone() {
                    for addr in listen_addrs {
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::RoutingUpdated {
                peer,
                ..
            })) => {
                tracing::info!("Kademlia routing updated for peer: {}", peer);
            }

            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(
                kad::Event::OutboundQueryProgressed {
                    result: kad::QueryResult::Bootstrap(Err(e)),
                    ..
                },
            )) => {
                tracing::error!("Kademlia bootstrap error: {:?}", e);
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                tracing::info!("Connected to peer: {}", peer_id);
            }
            _ => {}
        }
    }
}

// Fuck!
