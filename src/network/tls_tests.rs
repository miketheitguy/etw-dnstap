//! The TLS transport: versions, certificate checks, reconnection, and the
//! handshake deadline. Tests trust a throwaway CA through a `RootCertStore`;
//! the Windows platform verifier is not used here.

use super::test_support::{handshake_over, retry_policy};
use super::*;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    ProtocolVersion, RootCertStore, ServerConfig, ServerConnection, SupportedProtocolVersion,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::net::TcpListener;

/// A CA, and a server certificate it issued for `names`.
struct Pki {
    roots: RootCertStore,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

fn issue(names: &[&str]) -> Pki {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_key = KeyPair::generate().expect("CA key");
    let ca_certificate = ca_params.self_signed(&ca_key).expect("CA certificate");
    let issuer = Issuer::new(ca_params, ca_key);

    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    let server_params = CertificateParams::new(names).expect("server parameters");
    let server_key = KeyPair::generate().expect("server key");
    let server_certificate = server_params
        .signed_by(&server_key, &issuer)
        .expect("server certificate");

    let mut roots = RootCertStore::empty();
    roots.add(ca_certificate.der().clone()).expect("trusted CA");
    Pki {
        roots,
        chain: vec![server_certificate.der().clone()],
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn server_config(pki: &Pki, versions: &[&'static SupportedProtocolVersion]) -> Arc<ServerConfig> {
    Arc::new(
        ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(versions)
            .expect("server versions")
            .with_no_client_auth()
            .with_single_cert(pki.chain.clone(), pki.key.clone_key())
            .expect("server certificate"),
    )
}

const BOTH: &[&SupportedProtocolVersion] = &[&rustls::version::TLS12, &rustls::version::TLS13];

/// A client that trusts only the CA of `pki` and expects the loopback IP.
fn receiver_config(address: SocketAddr, pki: &Pki) -> ReceiverConfig {
    let client = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(BOTH)
        .expect("client versions")
        .with_root_certificates(pki.roots.clone())
        .with_no_client_auth();
    ReceiverConfig {
        address,
        tls: Some(TlsConfig {
            client: Arc::new(client),
            server_name: ServerName::IpAddress(TlsIpAddr::from(std::net::Ipv4Addr::LOCALHOST)),
        }),
    }
}

type ServerStream = StreamOwned<ServerConnection, TcpStream>;

/// Accepts one connection and starts the server side of TLS on it. The TLS
/// handshake runs on the first read.
fn accept_tls(listener: &TcpListener, config: &Arc<ServerConfig>) -> ServerStream {
    let (tcp, _) = listener.accept().expect("sender connects");
    tcp.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let connection = ServerConnection::new(Arc::clone(config)).expect("server connection");
    StreamOwned::new(connection, tcp)
}

enum Item {
    Data(Vec<u8>),
    Control(u32),
}

fn next_item(stream: &mut impl Read) -> Item {
    let mut length = [0; 4];
    stream.read_exact(&mut length).expect("frame length");
    if length == [0; 4] {
        let mut control = [0; 4];
        stream.read_exact(&mut control).expect("control length");
        let mut body = vec![0; u32::from_be_bytes(control) as usize];
        stream.read_exact(&mut body).expect("control body");
        return Item::Control(u32::from_be_bytes([body[0], body[1], body[2], body[3]]));
    }
    let mut payload = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut payload).expect("frame payload");
    Item::Data(payload)
}

/// Serves one full session (handshake, one data frame, STOP, FINISH) and returns
/// the negotiated protocol version.
fn serve_session(listener: &TcpListener, config: &Arc<ServerConfig>) -> ProtocolVersion {
    let mut stream = handshake_over(accept_tls(listener, config));
    assert!(matches!(next_item(&mut stream), Item::Data(payload) if payload == b"hello"));
    assert!(matches!(next_item(&mut stream), Item::Control(3)));
    write_control(&mut stream, CONTROL_FINISH, false).expect("FINISH");
    stream.conn.protocol_version().expect("negotiated version")
}

fn run_session(versions: &[&'static SupportedProtocolVersion]) -> ProtocolVersion {
    let pki = issue(&["127.0.0.1"]);
    let config = server_config(&pki, versions);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || serve_session(&listener, &config));
    let mut sender = Connection::connect(receiver_config(address, &pki)).expect("TLS session");
    sender.write_frame(b"hello").expect("frame");
    sender.finish().expect("STOP and FINISH over TLS");
    receiver.join().expect("receiver assertions pass")
}

#[test]
fn tls_handshake_frames_and_finish_over_tls13() {
    assert_eq!(run_session(BOTH), ProtocolVersion::TLSv1_3);
}

#[test]
fn tls12_only_receiver_negotiates_tls12() {
    assert_eq!(
        run_session(&[&rustls::version::TLS12]),
        ProtocolVersion::TLSv1_2
    );
}

#[test]
fn tls_server_name_mismatch_is_invalid_data_and_fatal_at_startup() {
    let pki = issue(&["other.example"]);
    let config = server_config(&pki, BOTH);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let mut stream = accept_tls(&listener, &config);
        // The client rejects the certificate, so this read fails.
        let mut byte = [0];
        let _ = stream.read(&mut byte);
    });
    let result = NetworkSink::start(receiver_config(address, &pki), Arc::default());
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidData));
    receiver.join().expect("receiver ends");
}

#[test]
fn plaintext_receiver_with_default_tls_is_invalid_data() {
    let pki = issue(&["127.0.0.1"]);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("sender connects");
        let mut stream = DeadlineTcp::new(tcp, Duration::from_secs(5));
        // Consume the ClientHello, answer in plaintext, then wait for the close
        // so the reply is not lost to a reset.
        let mut buffer = [0; 4096];
        let hello = stream.read(&mut buffer).expect("ClientHello");
        assert!(hello > 0);
        write_control(&mut stream, CONTROL_ACCEPT, true).expect("plaintext ACCEPT");
        while matches!(stream.read(&mut buffer), Ok(count) if count > 0) {}
    });
    let error = Connection::connect(receiver_config(address, &pki))
        .err()
        .expect("a plaintext reply is not TLS");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    receiver.join().expect("receiver ends");
}

#[test]
fn tls_reconnects_with_a_fresh_handshake() {
    let pki = issue(&["127.0.0.1"]);
    let config = server_config(&pki, BOTH);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let mut first = handshake_over(accept_tls(&listener, &config));
        assert!(matches!(next_item(&mut first), Item::Data(payload) if payload == b"one"));
        // The receiver goes away: the sender's session is dead.
        drop(first);
        let mut second = handshake_over(accept_tls(&listener, &config));
        loop {
            match next_item(&mut second) {
                Item::Data(payload) => assert_ne!(payload, b"one", "no resend"),
                Item::Control(3) => {
                    write_control(&mut second, CONTROL_FINISH, false).expect("FINISH");
                    break;
                }
                Item::Control(kind) => panic!("unexpected control {kind}"),
            }
        }
    });
    let stats = Arc::new(SinkStats::default());
    let mut sink = NetworkSink::start_with(
        receiver_config(address, &pki),
        Arc::clone(&stats),
        retry_policy(10, 40),
    )
    .expect("connects");
    sink.write_frame(b"one").expect("write");
    sink.flush().expect("flush");
    let give_up = Instant::now() + Duration::from_secs(20);
    while stats.reconnects.load(Ordering::Relaxed) == 0 {
        assert!(Instant::now() < give_up, "sink never reconnected");
        sink.write_frame(b"offered").expect("write");
        sink.flush().expect("flush");
        thread::sleep(Duration::from_millis(5));
    }
    sink.write_frame(b"after").expect("write");
    sink.finish()
        .expect("clean STOP/FINISH on the new TLS session");
    receiver.join().expect("receiver assertions pass");
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 1);
    assert_eq!(stats.reconnects.load(Ordering::Relaxed), 1);
}

#[test]
fn tls_handshake_respects_the_deadline() {
    let pki = issue(&["127.0.0.1"]);
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let (release, released) = mpsc::channel::<()>();
    let receiver = thread::spawn(move || {
        // Accept, then say nothing until the sender has given up.
        let (_tcp, _) = listener.accept().expect("sender connects");
        let _ = released.recv_timeout(Duration::from_secs(10));
    });
    let started = Instant::now();
    let error = Connection::connect(receiver_config(address, &pki))
        .err()
        .expect("a silent receiver cannot complete a handshake");
    let elapsed = started.elapsed();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(elapsed >= IO_TIMEOUT / 2, "{elapsed:?}");
    assert!(elapsed < IO_TIMEOUT * 3, "{elapsed:?}");
    release.send(()).expect("release receiver");
    receiver.join().expect("receiver ends");
}
