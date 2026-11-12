//! Authenticated loopback SOCKS5 tunnel; the TLS server still verifies the forge exchange.
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub(super) struct SocksFixture {
    address: SocketAddr,
    calls: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for SocksFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl SocksFixture {
    pub async fn new(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let count = count.clone();
                tokio::spawn(async move {
                    tokio::time::timeout(Duration::from_secs(10), forward(stream, target, &count))
                        .await
                        .expect("SOCKS fixture tunnel must settle");
                });
            }
        });
        Self {
            address,
            calls,
            server,
        }
    }

    pub fn url(&self, scheme: &str) -> String {
        format!("{scheme}://proxy-user:proxy-password@{}", self.address)
    }

    pub fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

async fn forward(mut client: TcpStream, target: SocketAddr, count: &AtomicUsize) {
    assert_eq!(client.read_u8().await.unwrap(), 5);
    let methods = read_field(&mut client).await;
    assert!(
        methods.contains(&2),
        "client must offer username/password authentication"
    );
    client.write_all(&[5, 2]).await.unwrap();
    assert_eq!(client.read_u8().await.unwrap(), 1);
    assert_eq!(read_field(&mut client).await, b"proxy-user");
    assert_eq!(read_field(&mut client).await, b"proxy-password");
    client.write_all(&[1, 0]).await.unwrap();

    let mut request = [0; 4];
    client.read_exact(&mut request).await.unwrap();
    assert_eq!(&request[..3], &[5, 1, 0]);
    match request[3] {
        1 => {
            let mut ip = [0; 4];
            client.read_exact(&mut ip).await.unwrap();
            assert_eq!(std::net::IpAddr::V4(ip.into()), target.ip());
        }
        3 => assert_eq!(
            read_field(&mut client).await,
            target.ip().to_string().as_bytes()
        ),
        kind => panic!("unexpected SOCKS address type {kind}"),
    }
    assert_eq!(client.read_u16().await.unwrap(), target.port());
    let mut upstream = TcpStream::connect(target).await.unwrap();
    count.fetch_add(1, Ordering::SeqCst);
    client
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
        .await
        .unwrap();
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

async fn read_field(client: &mut TcpStream) -> Vec<u8> {
    let mut bytes = vec![0; usize::from(client.read_u8().await.unwrap())];
    client.read_exact(&mut bytes).await.unwrap();
    bytes
}
