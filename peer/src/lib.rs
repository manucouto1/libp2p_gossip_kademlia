use anyhow::Result;
use futures::{StreamExt, channel::mpsc};
use libp2p::{
    PeerId, Swarm,
    core::multiaddr::Multiaddr,
    dcutr, gossipsub, identify, identity,
    kad::{self, BootstrapOk, Mode, store::MemoryStore},
    multiaddr::Protocol,
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::Duration,
};
use tokio::task;
use tokio::{select, sync::Mutex, time::interval};

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    dcutr: dcutr::Behaviour,
}
#[derive(Clone)]
pub enum PeerEvent {
    MessageReceived(String),
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
}

pub struct Peer {
    peer_id: PeerId,
    relay_peer_id: PeerId,
    relay_address: Multiaddr,
    swarm: Arc<Mutex<Swarm<Behaviour>>>,
    message_sender: Arc<Mutex<mpsc::UnboundedSender<String>>>,
    message_receiver: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
}

impl Peer {
    pub async fn new(secret_key_seed: u8, relay_address: Multiaddr) -> Result<Self> {
        let local_key = generate_ed25519(secret_key_seed);
        let peer_id = PeerId::from(local_key.public());

        let (message_sender, message_receiver) = mpsc::unbounded::<String>();

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(std::time::Duration::from_secs(10))
            .validation_mode(gossipsub::ValidationMode::Permissive)
            .message_id_fn(|message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                (message.data.clone(), now).hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            })
            .build()
            .unwrap();

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        );

        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|keypair, relay_behaviour| Behaviour {
                kademlia: kad::Behaviour::new(
                    keypair.public().to_peer_id(),
                    MemoryStore::new(keypair.public().to_peer_id()),
                ),
                gossipsub: gossipsub.unwrap(),
                relay_client: relay_behaviour,
                ping: ping::Behaviour::new(ping::Config::new()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/TODO/0.0.1".to_string(),
                    keypair.public(),
                )),
                dcutr: dcutr::Behaviour::new(keypair.public().to_peer_id()),
            })?
            .build();

        swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Client));

        let relay_peer_id = relay_address
            .iter()
            .find_map(|p| match p {
                Protocol::P2p(peer_id) => Some(peer_id),
                _ => None,
            })
            .expect("Invalid relay address");

        swarm
            .behaviour_mut()
            .kademlia
            .add_address(&relay_peer_id, relay_address.clone());

        swarm
            .behaviour_mut()
            .kademlia
            .bootstrap()
            .expect("Failed to bootstrap Kademlia");

        swarm
            .listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse().unwrap())
            .unwrap();
        swarm
            .listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap())
            .unwrap();

        tracing::info!("Trying to dial relay: {:?}", relay_address);
        swarm.dial(relay_address.clone()).unwrap();
        tracing::info!("Relay dialed!");

        swarm
            .listen_on(relay_address.clone().with(Protocol::P2pCircuit))
            .unwrap();

        Ok(Peer {
            peer_id,
            relay_peer_id,
            relay_address,

            swarm: Arc::new(Mutex::new(swarm)), //Arc<RwLock<Swarm<Behaviour>>>
            message_sender: Arc::new(Mutex::new(message_sender)),
            message_receiver: Arc::new(Mutex::new(message_receiver)),
        })
    }

    pub fn run(&mut self, topic: String) {
        let topic = gossipsub::IdentTopic::new(topic);
        let swarm = Arc::clone(&self.swarm);
        let message_receiver = Arc::clone(&self.message_receiver);
        let _relay_address = self.relay_address.clone();
        let relay_peer_id = self.relay_peer_id.clone();
        let self_peer_id = self.peer_id.clone();
        let mut interval = interval(Duration::from_secs(30));

        task::spawn(async move {
            swarm
                .lock()
                .await
                .behaviour_mut()
                .gossipsub
                .subscribe(&topic)
                .expect("Failed to subscribe to topic");

            tracing::info!("Subscribed to topic: {:?}", topic);

            loop {
                let mut locked_message_receiver = message_receiver.lock().await;
                let mut locked_swarm = swarm.lock().await;

                select! {
                    _ = interval.tick() => {
                        // tracing::info!("Trying to dial relay: {:?}", relay_address);
                        // locked_swarm.dial(relay_address.clone()).unwrap();
                        // tracing::info!("Relay dialed!");

                        // locked_swarm
                        //     .listen_on(relay_address.clone().with(Protocol::P2pCircuit))
                        //     .unwrap();

                        // let gossip_peers = locked_swarm.behaviour().gossipsub.mesh_peers(&topic.hash());
                        // for peer in gossip_peers {
                        //     tracing::info!("Discovered peer: {:?}", peer);
                        // }
                    }
                    message = locked_message_receiver.next() => {
                        match message {
                            Some(message) => {
                                println!("> {:?}", message);

                                match locked_swarm
                                    .behaviour_mut()
                                    .gossipsub
                                    .publish(topic.clone(), message.as_bytes()) {
                                        Ok(_) => println!("Message published"),
                                        Err(e) => tracing::error!("Publish error: {:?}", e),
                                    }

                            }
                            None => {
                                tracing::error!("Message receiver channel closed");
                            }
                        }
                    },
                    event = locked_swarm.select_next_some() => match event {
                        SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                            peer_id,
                            info: identify::Info { listen_addrs, .. },
                            ..
                        })) => {
                            for addr in listen_addrs {
                                locked_swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                            }
                            if let Err(e) = locked_swarm.behaviour_mut().kademlia.bootstrap() {
                                tracing::error!("Failed to bootstrap Kademlia: {:?}", e);
                            }
                        }
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
                                    locked_swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                                }
                                e => { tracing::info!("Gossip Event: {e:?}")}
                            }

                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad_event)) =>  {
                            match kad_event {
                                // kad::Event::RoutingUpdated { peer, .. } => {
                                //     tracing::info!("Kademlia: Routing table updated for peer: {:?}", peer);
                                // }
                                kad::Event::OutboundQueryProgressed { result, .. } => {
                                    match result {
                                        kad::QueryResult::GetClosestPeers(Ok(ok)) => {
                                            tracing::info!("Kademlia: Found closest peers");
                                            for peer in ok.peers {
                                                if peer.peer_id!= self_peer_id && peer.peer_id!= relay_peer_id {
                                                    tracing::info!("Discovered peer: {:?}", peer);
                                                }
                                                // locked_swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer.peer_id);
                                            }
                                        }
                                        kad::QueryResult::Bootstrap(Ok(BootstrapOk { peer, num_remaining: _ })) => {
                                            if peer != self_peer_id && peer != relay_peer_id{
                                                tracing::info!("Bootstrapped with peer: {:?}", peer);
                                                // locked_swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                                            }
                                        }
                                        _ => {}
                                    }
                                }

                                _ => {}
                            }
                        }
                        _ => { }
                    }

                }
            }
        });
    }

    pub async fn send_message(&self, message: String) {
        Arc::clone(&self.message_sender)
            .lock()
            .await
            .unbounded_send(message)
            .expect("Failed to send message");
    }

    pub async fn get_closest_peers(&self) {
        Arc::clone(&self.swarm)
            .lock()
            .await
            .behaviour_mut()
            .kademlia
            .get_closest_peers(self.peer_id);
    }
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;
    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}
