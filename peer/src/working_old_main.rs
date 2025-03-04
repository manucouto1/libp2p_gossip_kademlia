use std::{
    error::Error,
    hash::{DefaultHasher, Hash, Hasher},
    thread::sleep,
    time::Duration,
};

use clap::Parser;
use futures::stream::StreamExt;
use libp2p::{
    core::multiaddr::Multiaddr,
    gossipsub, identify, identity,
    kad::{self, BootstrapOk, GetProvidersOk, Mode, store::MemoryStore},
    multiaddr::Protocol,
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::{io, io::AsyncBufReadExt, select};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[clap(name = "libp2p DCUtR client")]
struct Opts {
    /// Fixed value to generate deterministic peer id.
    #[clap(long)]
    secret_key_seed: u8,

    /// The listening address
    #[clap(long)]
    relay_address: Multiaddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opts = Opts::parse();

    let local_key = generate_ed25519(opts.secret_key_seed);

    #[derive(NetworkBehaviour)]
    struct Behaviour {
        relay_client: relay::client::Behaviour,
        ping: ping::Behaviour,
        identify: identify::Behaviour,
        gossipsub: gossipsub::Behaviour,
        kademlia: kad::Behaviour<MemoryStore>,
    }

    // 3. Configurar Gossipsub
    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(std::time::Duration::from_secs(10))
        .validation_mode(gossipsub::ValidationMode::Permissive)
        .message_id_fn(|message: &gossipsub::Message| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        })
        .build()
        .unwrap();

    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )?;

    let topic = gossipsub::IdentTopic::new("private-chat");

    gossipsub.subscribe(&topic)?;

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|keypair, relay_behaviour| Behaviour {
            kademlia: kad::Behaviour::new(
                keypair.public().to_peer_id(),
                MemoryStore::new(keypair.public().to_peer_id()),
            ),
            gossipsub: gossipsub,
            relay_client: relay_behaviour,
            ping: ping::Behaviour::new(ping::Config::new()),
            identify: identify::Behaviour::new(identify::Config::new(
                "/TODO/0.0.1".to_string(),
                keypair.public(),
            )),
        })?
        .build();

    swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Client));

    let relay_peer_id = opts
        .relay_address
        .iter()
        .find_map(|p| match p {
            Protocol::P2p(peer_id) => Some(peer_id),
            _ => None,
        })
        .expect("La dirección de relay no válida");

    swarm
        .behaviour_mut()
        .kademlia
        .add_address(&relay_peer_id, opts.relay_address.clone());
    swarm
        .behaviour_mut()
        .kademlia
        .bootstrap()
        .expect("Error al iniciar el bootstrap de Kademlia");

    swarm
        .listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse().unwrap())
        .unwrap();
    swarm
        .listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap())
        .unwrap();

    swarm.dial(opts.relay_address.clone()).unwrap();

    swarm
        .listen_on(opts.relay_address.clone().with(Protocol::P2pCircuit))
        .unwrap();

    sleep(Duration::from_secs(10));
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if let Err(e) = swarm
                    .behaviour_mut().gossipsub
                    .publish(topic.clone(), line.as_bytes()) {
                    tracing::error!("Publish error: {e:?}");
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(
                    gossip_event
                )) => {
                    match gossip_event {
                        gossipsub::Event::Message { message, .. } => {
                            tracing::info!(
                                "Mensaje recibido: {}",
                                String::from_utf8_lossy(&message.data)
                            )
                        }
                        gossipsub::Event::Subscribed { peer_id, topic } => {
                            tracing::info!("Peer {:?} se suscribió al tema {:?}", peer_id, topic);
                        }
                        gossipsub::Event::Unsubscribed { peer_id, topic } => {
                            tracing::info!("Peer {:?} se desuscribió del tema {:?}", peer_id, topic);
                        }
                        gossipsub::Event::GossipsubNotSupported { peer_id } => {
                            tracing::warn!("Peer does not support Gossipsub: {:?}", peer_id);
                            // Opcionalmente, puedes remover este peer de la lista de peers de Gossipsub
                            swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        }
                        e => { tracing::info!("Gossip Event: {e:?}")}
                    }

                }
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    if peer_id == relay_peer_id {
                        tracing::warn!("Connection to relay closed. Attempting to reconnect...");
                        swarm.dial(opts.relay_address.clone()).unwrap();
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad_event)) =>  {
                    match kad_event {
                        kad::Event::RoutingUpdated { peer, .. } => {
                            tracing::info!("Kademlia: Routing table updated for peer: {:?}", peer);
                        }
                        kad::Event::OutboundQueryProgressed { result, .. } => {
                            match result {
                                kad::QueryResult::GetClosestPeers(Ok(ok)) => {
                                    tracing::info!("Kademlia: Found closest peers");
                                    for peer in ok.peers {
                                        tracing::info!("Discovered peer: {:?}", peer);
                                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer.peer_id);
                                    }
                                }
                                kad::QueryResult::Bootstrap(Ok(BootstrapOk { peer, num_remaining: _ })) => {
                                    tracing::info!("Kademlia: Bootstrap query completed");
                                    tracing::info!("Bootstrapped with peer: {:?}", peer);
                                    swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                                }
                                kad::QueryResult::GetProviders(Ok(ok)) => {
                                    tracing::info!("Kademlia: Found providers");
                                    match ok {
                                        GetProvidersOk::FoundProviders { key, providers } => {
                                            tracing::info!("Kademlia: Found providers for key: {:?}", key);
                                            for peer in providers {
                                                tracing::info!("Provider peer: {:?}", peer);
                                                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                                            }
                                        }
                                        GetProvidersOk::FinishedWithNoAdditionalRecord { closest_peers } => {
                                            tracing::info!("Kademlia: No additional providers found");
                                            tracing::info!("Closest peers: {:?}", closest_peers);
                                            for peer in closest_peers {
                                                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                                            }
                                        }
                                    }
                                }
                                // Manejar otros tipos de QueryResult según sea necesario
                                _ => tracing::info!("Other Kademlia query result: {:?}", result),
                            }
                        }
                        _ => {tracing::info!("Outbound query progress: {:?}", kad_event); }
                    }
                }

                e => { tracing::info!("Event: {e:?}")}
            }
        }
    }
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}
