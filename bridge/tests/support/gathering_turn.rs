//! Loopback-only TURN allocation fixture. It gathers real relay candidates but
//! deliberately does not forward packets or establish a working TURN data path.
use std::net::SocketAddr;
use std::time::Duration;

use rtc::stun::attributes::{ATTR_NONCE, ATTR_REALM};
use rtc::stun::error_code::CODE_UNAUTHORIZED;
use rtc::stun::message::{
    Message, MessageType, CLASS_ERROR_RESPONSE, CLASS_SUCCESS_RESPONSE, METHOD_ALLOCATE,
    METHOD_CREATE_PERMISSION, METHOD_REFRESH,
};
use rtc::stun::textattrs::{Nonce, Realm};
use rtc::turn::proto::lifetime::Lifetime;
use rtc::turn::proto::relayaddr::RelayedAddress;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

pub struct GatheringTurn {
    pub url: String,
    pub relay_addr: SocketAddr,
    task: JoinHandle<()>,
}

impl GatheringTurn {
    pub async fn start() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        // Advertise an actual reserved loopback socket, never a public address.
        let relay_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let relay_addr = relay_socket.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let _relay_socket = relay_socket;
            let mut buf = [0; 2048];
            while let Ok((n, peer)) = socket.recv_from(&mut buf).await {
                let mut request = Message::new();
                request.raw = buf[..n].to_vec();
                if request.decode().is_err() {
                    continue;
                }
                if let Some(response) = respond(&request, relay_addr) {
                    socket.send_to(&response.raw, peer).await.unwrap();
                }
            }
        });
        Self {
            url: format!("turn:{addr}?transport=udp"),
            relay_addr,
            task,
        }
    }
}

impl Drop for GatheringTurn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn respond(request: &Message, relay_addr: SocketAddr) -> Option<Message> {
    let mut response = Message::new();
    let method = request.typ.method;
    if method == METHOD_ALLOCATE && request.get(ATTR_NONCE).is_err() {
        response
            .build(&[
                Box::new(request.transaction_id),
                Box::new(MessageType::new(method, CLASS_ERROR_RESPONSE)),
                Box::new(CODE_UNAUTHORIZED),
                Box::new(Realm::new(ATTR_REALM, "test-only".to_owned())),
                Box::new(Nonce::new(ATTR_NONCE, "test-nonce".to_owned())),
            ])
            .unwrap();
    } else if method == METHOD_ALLOCATE {
        response
            .build(&[
                Box::new(request.transaction_id),
                Box::new(MessageType::new(method, CLASS_SUCCESS_RESPONSE)),
                Box::new(RelayedAddress {
                    ip: relay_addr.ip(),
                    port: relay_addr.port(),
                }),
                Box::new(Lifetime(Duration::from_secs(600))),
            ])
            .unwrap();
    } else if method == METHOD_REFRESH || method == METHOD_CREATE_PERMISSION {
        response
            .build(&[
                Box::new(request.transaction_id),
                Box::new(MessageType::new(method, CLASS_SUCCESS_RESPONSE)),
                Box::new(Lifetime(Duration::from_secs(600))),
            ])
            .unwrap();
    } else {
        return None;
    }
    Some(response)
}
