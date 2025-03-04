use futures::{executor::block_on, future::FutureExt, stream::StreamExt};
use std::{
    collections::hash_map::DefaultHasher,
    error::Error,
    hash::{Hash, Hasher},
    str::FromStr,
    time::Duration,
};

use libp2p::{
    Multiaddr, PeerId, dcutr, gossipsub, identify, mdns, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::{io, io::AsyncBufReadExt, select};
use tracing_subscriber::EnvFilter;

// We create a custom network behaviour that combines Gossipsub and Mdns.
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    relay_client: relay::client::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    dcutr: dcutr::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Tracing initialized");

    let relay_peer_id = PeerId::from_str("12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X")?;

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|keypair, relay_behaviour| {
            // To content-address message, we can take the hash of message and use it as an ID.
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10)) // This is set to aid debugging by not cluttering the log space
                .validation_mode(gossipsub::ValidationMode::Strict) // This sets the kind of message validation. The default is Strict (enforce message
                // signing)
                .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?; // Temporary hack because `build` does not return a proper `std::error::Error`.

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(keypair.clone()),
                gossipsub_config,
            )?;

            let mdns = mdns::tokio::Behaviour::new(
                mdns::Config::default(),
                keypair.public().to_peer_id(),
            )?;

            Ok(MyBehaviour {
                gossipsub: gossipsub,
                mdns: mdns,
                relay_client: relay_behaviour,
                ping: ping::Behaviour::new(ping::Config::new()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/TODO/0.0.1".to_string(),
                    keypair.public(),
                )),
                dcutr: dcutr::Behaviour::new(keypair.public().to_peer_id()),
            })
        })?
        .build();

    // Create a Gossipsub topic
    let topic = gossipsub::IdentTopic::new("test-net");
    // subscribes to our topic
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    let relay_address: Multiaddr =
        "/ip4/172.20.0.100/tcp/4000/p2p/12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X"
            .parse()
            .map_err(|_| "Formato de relay inválido")?;

    match swarm.dial(relay_address) {
        Ok(_) => {
            println!("Connection with relay established!");
            tracing::info!("Successfully dialed relay");
        }
        Err(e) => {
            println!("Error connecting to relay: {:?}", e);
            tracing::error!("Failed to dial relay: {:?}", e);
        }
    }

    // Añade esto para ver los peers conocidos
    println!(
        "Known peers: {:?}",
        swarm.behaviour().gossipsub.all_peers().collect::<Vec<_>>()
    );

    println!(
        "> {:?}",
        match block_on(async {
            let mut delay = futures_timer::Delay::new(std::time::Duration::from_secs(30)).fuse();
            let mut listen_addr_found = false;
            let mut relay_connected = false;

            loop {
                futures::select! {
                    event = swarm.next() => {
                        if let Some(event) = event {
                            match event {
                                SwarmEvent::NewListenAddr { address, .. } => {
                                    tracing::info!(%address, "Listening on address");
                                    listen_addr_found = true;
                                }
                                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                                    if peer_id == relay_peer_id {  // Asume que tienes el PeerId del relay
                                        tracing::info!("Connected to relay");
                                        relay_connected = true;
                                    }
                                }
                                SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Sent {
                                    ..
                                })) => {
                                    tracing::info!("Told relay its public address");
                                }
                                SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Received {
                                    info: identify::Info { observed_addr, .. },
                                    ..
                                })) => {
                                    tracing::info!(address=%observed_addr, "Relay told us our observed address");
                                }
                                _ => {
                                    tracing::debug!("Received other event: {:?}", event);
                                }
                            }
                        }
                    }
                    _ = &mut delay => {
                        tracing::warn!("Timeout waiting for listen address and relay connection");
                        break;
                    }
                }

                if listen_addr_found && relay_connected {
                    tracing::info!("Listen address found and connected to relay. Continuing...");
                    break;
                }
            }
            if !listen_addr_found || !relay_connected {
                return Err("Failed to initialize network properly");
            }
            Ok(())
        }) {
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            Ok(()) => {}
        }
    );

    println!("Enter messages via STDIN and they will be sent to connected peers using Gossipsub");

    // Kick it off
    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if let Err(e) = swarm
                    .behaviour_mut().gossipsub
                    .publish(topic.clone(), line.as_bytes()) {
                    println!("Publish error: {e:?}");
                }

                // swarm.relay_client
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, _multiaddr) in list {
                        println!("mDNS discovered a new peer: {peer_id}");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    }
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _multiaddr) in list {
                        println!("mDNS discover peer has expired: {peer_id}");
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    }
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => println!(
                        "Got message: '{}' with id: {id} from peer: {peer_id}",
                        String::from_utf8_lossy(&message.data),
                    ),
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Local node is listening on {address}");
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::RelayClient(event)) => {
                    match event {
                        relay::client::Event::ReservationReqAccepted { .. } => {
                            // Peer ha reservado un slot en el relay
                            println!("¡Reserva en relay confirmada!");
                        }
                        _ => {}
                    }
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Identify(event)) => {
                    if let identify::Event::Received { peer_id, info, connection_id } = event {
                        println!("Peer {} tiene direcciones: {:?} ", peer_id, info.listen_addrs);
                        println!("Connection ID: {} {:?}", peer_id, connection_id);
                    }
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Dcutr(event)) => {
                    println!("> Nuevo evento en DCUtR: {:?}", event);
                    // match event {
                    //     dcutr::Event::OutboundSuccess { remote_peer_id } => {
                    //         println!("¡Conexión directa establecida con {}!", remote_peer_id);
                    //     }
                    //     dcutr::Event::InboundSuccess { remote_peer_id } => {
                    //         println!("¡Conexión directa establecida con {}!", remote_peer_id);
                    //     }
                    //     dcutr::Event::RemoteProtocolError { .. } => {
                    //         eprintln!("Error en DCUtR");
                    //     }
                    // }
                }
                _ => {
                    println!("Received other event: {:?}", event);
                }
            }
        }
    }
}
