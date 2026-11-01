//! Format-neutral decoded DNS Server events, shared by the ETW decoder and the
//! output encoders so neither depends on the other.

use std::{net::IpAddr, time::SystemTime};

/// DNS transport reported by the ETW provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Transport {
    Udp,
    Tcp,
}

/// Metadata actually observed in one DNS Server event; packet storage is separate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DnsEventMetadata {
    pub(crate) event_id: u16,
    pub(crate) timestamp: SystemTime,
    pub(crate) transport: Transport,
    pub(crate) peer_address: Option<IpAddr>,
    pub(crate) peer_port: Option<u16>,
    pub(crate) local_address: Option<IpAddr>,
    pub(crate) local_port: Option<u16>,
}

/// One decoded event: observed metadata plus the borrowed DNS wire packet. The
/// packet borrows decoder-owned storage, so a view lives only until the caller
/// releases the event or decoder it came from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DecodedDnsEvent<'a> {
    pub(crate) metadata: DnsEventMetadata,
    pub(crate) packet: &'a [u8],
}
