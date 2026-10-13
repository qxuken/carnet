//! Apps of other builds against a running server (docs/SYNC.md §14): one
//! from before protocols is greeted and turned away with a reason it can
//! act on; one of a later protocol has the frames this server doesn't
//! know skipped, not hung up on; and `/v1/info` says it all before a
//! socket opens.

use std::path::PathBuf;
use std::time::Duration;

use carnet_server::server::{self, Config};
use carnet_sync::compat;
use carnet_sync::wire::{self, Carnet, Deny, Frame};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

fn temp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-compat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

/// Every Carnet frame until the socket closes or goes quiet.
async fn frames(ws: &mut Socket) -> Vec<Carnet> {
    let mut got = Vec::new();
    while let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
        if let Message::Binary(b) = m
            && let Ok(Frame::Carnet(c)) = wire::decode(&b)
        {
            got.push(c);
        }
    }
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apps_of_other_builds_are_told_where_they_stand() {
    let data = temp();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None };
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let done = tokio::spawn(server::serve(config, listener, async {
        let _ = rx.await;
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;

    // An app from before protocols: no `?protocol=`.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/sync")).await.unwrap();
    let got = frames(&mut ws).await;
    match &got[..] {
        [Carnet::Challenge { server, .. }, Carnet::Denied { reason, why: Deny::Outdated }] => {
            assert_eq!((server.protocol, server.oldest), (compat::PROTOCOL, compat::OLDEST));
            assert!(server.has(compat::feature::PASSKEYS) && server.has(compat::feature::ACCOUNT_DELETE));
            assert!(!server.has(compat::feature::PASSWORDS), "not without --passwords");
            assert_eq!(reason, compat::APP_TOO_OLD);
        }
        other => panic!("{other:?}"),
    }

    // This build, sending a later protocol's frame first: skipped, and
    // what follows is answered (here, an invite that isn't one).
    let (mut ws, _) = tokio_tungstenite::connect_async(compat::with_protocol(&format!("ws://{addr}/v1/sync"))).await.unwrap();
    ws.send(Message::Binary(b"%CRN\xc8later".to_vec().into())).await.unwrap();
    let enroll = Carnet::Enroll { invite: "AAAA-AAAA-AAAA-AAAA".into(), device: b"d".to_vec(), public: [1; 32] };
    ws.send(Message::Binary(wire::encode(&Frame::Carnet(enroll)).into())).await.unwrap();
    let got = frames(&mut ws).await;
    assert!(matches!(got.first(), Some(Carnet::Challenge { .. })), "{got:?}");
    match got.last() {
        Some(Carnet::Denied { reason, why: Deny::Other }) => assert_ne!(reason, "not a frame", "the later frame was skipped"),
        other => panic!("{other:?}"),
    }

    // Before any socket: the same, over HTTP.
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET /v1/info HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut info = String::new();
    s.read_to_string(&mut info).await.unwrap();
    for want in [format!("\"protocol\":{}", compat::PROTOCOL), format!("\"oldest\":{}", compat::OLDEST), "\"features\":[\"passkeys\"".into()] {
        assert!(info.contains(&want), "{want} in {info}");
    }
    assert!(!info.contains("\"signup\""), "a server of one's own takes no signups: {info}");

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}
