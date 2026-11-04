//! Helpers shared by the network test modules, and the TLS test kit that the
//! sender tests use too.

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    RootCertStore, ServerConfig, ServerConnection,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};

use super::*;

/// Completes the receiver side of the Frame Streams handshake (READY, ACCEPT,
/// START) and returns the stream for the rest of the exchange.
pub(super) fn handshake(stream: TcpStream) -> DeadlineTcp {
    handshake_over(DeadlineTcp::new(stream, Duration::from_secs(10)))
}

/// The receiver side of the handshake over any stream, such as a TLS server
/// connection.
pub(super) fn handshake_over<S: Read + Write>(mut stream: S) -> S {
    read_control(&mut stream, CONTROL_READY, true).expect("READY");
    write_control(&mut stream, CONTROL_ACCEPT, true).expect("ACCEPT");
    read_control(&mut stream, 2, true).expect("START");
    stream
}

/// A loopback port with no listener, so a connection to it is refused, held
/// until the value is dropped.
///
/// The port is the client end of a loopback connection. While that connection
/// is open, no other socket is given the port, so a test running in parallel
/// cannot bind it and accept the connection. A port taken from a dropped
/// listener has no such guard.
pub(super) struct RefusedPort {
    address: SocketAddr,
    _connection: (TcpStream, TcpStream),
}

impl RefusedPort {
    pub(super) fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let client =
            TcpStream::connect(listener.local_addr().expect("listener address")).expect("connect");
        let (server, _) = listener.accept().expect("accept");
        Self {
            address: client.local_addr().expect("client address"),
            _connection: (client, server),
        }
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }
}

/// A reconnect policy with the given delays in milliseconds. Use a small pair to
/// reconnect quickly, or a very large one so no retry happens within a test.
pub(super) fn retry_policy(initial_ms: u64, max_ms: u64) -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_millis(initial_ms),
        max: Duration::from_millis(max_ms),
    }
}

/// How long [`wait_until`] waits before failing. Generous: the reconnect
/// policies in these tests retry within tens of milliseconds, and the bound
/// only matters when a test fails.
pub(super) const WAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Polls `condition` until it holds. Fails the test when `timeout` elapses
/// first, naming `what` was waited for. The condition may do work on each poll,
/// such as offering frames to a sink so it notices a reconnect.
pub(super) fn wait_until(timeout: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for {what}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// A CA and a server certificate for `127.0.0.1`, for in-memory TLS sessions.
pub(crate) struct TlsKit {
    client: Arc<ClientConfig>,
    server: Arc<ServerConfig>,
}

impl TlsKit {
    pub(crate) fn new() -> Arc<Self> {
        Self::with_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
    }

    /// A kit whose sessions negotiate one of `versions`.
    pub(crate) fn with_versions(
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> Arc<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca_key = KeyPair::generate().expect("CA key");
        let ca_certificate = ca_params.self_signed(&ca_key).expect("CA certificate");
        let issuer = Issuer::new(ca_params, ca_key);
        let server_params =
            CertificateParams::new(vec!["127.0.0.1".to_owned()]).expect("server parameters");
        let server_key = KeyPair::generate().expect("server key");
        let server_certificate = server_params
            .signed_by(&server_key, &issuer)
            .expect("server certificate");
        let mut roots = RootCertStore::empty();
        roots.add(ca_certificate.der().clone()).expect("trusted CA");
        let chain: Vec<CertificateDer<'static>> = vec![server_certificate.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der()));
        let client = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(versions)
            .expect("client versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .expect("server versions")
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .expect("server certificate");
        Arc::new(Self {
            client: Arc::new(client),
            server: Arc::new(server),
        })
    }

    /// A client connection whose handshake with a fresh server side is done in
    /// memory, and that server side.
    pub(crate) fn pair(&self) -> (ClientConnection, ServerConnection) {
        let mut client = ClientConnection::new(
            Arc::clone(&self.client),
            ServerName::IpAddress(TlsIpAddr::from(std::net::Ipv4Addr::LOCALHOST)),
        )
        .expect("client connection");
        let mut server =
            ServerConnection::new(Arc::clone(&self.server)).expect("server connection");
        for _ in 0..32 {
            let mut to_server = Vec::new();
            while client.wants_write() {
                client.write_tls(&mut to_server).expect("client records");
            }
            let mut rest = &to_server[..];
            while !rest.is_empty() {
                server.read_tls(&mut rest).expect("server reads records");
                server.process_new_packets().expect("server processes");
            }
            let mut to_client = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut to_client).expect("server records");
            }
            let mut reply = &to_client[..];
            while !reply.is_empty() {
                client.read_tls(&mut reply).expect("client reads records");
                client.process_new_packets().expect("client processes");
            }
            if !client.is_handshaking() && !server.is_handshaking() && !client.wants_write() {
                break;
            }
        }
        assert!(!client.is_handshaking(), "the in-memory handshake finished");
        (client, server)
    }
}
