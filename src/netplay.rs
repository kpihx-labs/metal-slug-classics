use crate::profile::NetplayRole;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_DISCOVERY_PORT: u16 = 55_434;
pub const DEFAULT_GAME_PORT: u16 = 55_435;
pub const DEFAULT_MULTICAST_ADDRESS: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 77);
pub const SESSION_PROTOCOL_VERSION: u32 = 3;
pub const MAX_PACKET_BYTES: usize = 16_384;
const DISCOVERY_MAGIC: &str = "metal-slug-classics-lan";
const DISCOVERY_VERSION: u32 = 1;
const MAX_ADVERTISEMENT_TTL_MS: u64 = 60_000;
const MAX_CLOCK_SKEW_MS: u64 = 30_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionContract {
    pub protocol_version: u32,
    pub game: String,
    pub manifest_id: String,
    pub manifest_content_sha256: String,
    pub runtime_compatibility_id: String,
    pub core_compatibility_id: Option<String>,
    pub rom_sha256: String,
    pub bios_sha256: String,
    pub language: String,
    pub region: String,
    pub behavior_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractMismatch {
    pub field: String,
    pub local: String,
    pub peer: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgreementReport {
    pub agreed: bool,
    pub mismatches: Vec<ContractMismatch>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryPacket {
    pub magic: String,
    pub discovery_version: u32,
    pub session_id: String,
    pub sent_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub started_at_unix_ms: u64,
    pub reconnect_until_unix_ms: u64,
    pub game_port: u16,
    pub contract: SessionContract,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveredSession {
    pub source: SocketAddr,
    pub session_id: String,
    pub started_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub reconnect_until_unix_ms: u64,
    pub game_port: u16,
    pub contract: SessionContract,
}

#[derive(Clone, Debug)]
pub struct DiscoveryOptions {
    pub interfaces: Vec<Ipv4Addr>,
    pub discovery_port: u16,
    pub multicast_address: Ipv4Addr,
    pub timeout: Duration,
    pub allow_public: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            interfaces: Vec::new(),
            discovery_port: DEFAULT_DISCOVERY_PORT,
            multicast_address: DEFAULT_MULTICAST_ADDRESS,
            timeout: Duration::from_secs(3),
            allow_public: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LocalIpv4Interface {
    address: Ipv4Addr,
    netmask: Ipv4Addr,
    prefix_length: u8,
    broadcast: Option<Ipv4Addr>,
}

#[must_use]
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[must_use]
pub fn is_private_or_local(address: Ipv4Addr) -> bool {
    address.is_private() || address.is_loopback() || address.is_link_local()
}

#[must_use]
fn is_private_lan(address: Ipv4Addr) -> bool {
    address.is_private() || address.is_link_local()
}

/// Lists usable local IPv4 addresses. Public addresses are excluded unless explicitly allowed.
///
/// # Errors
/// Returns an error when interfaces cannot be enumerated, the default route is unusable, or an
/// explicit address is unavailable.
pub fn local_interfaces(requested: &[Ipv4Addr], allow_public: bool) -> Result<Vec<Ipv4Addr>> {
    Ok(selected_local_interfaces(requested, allow_public)?
        .into_iter()
        .map(|interface| interface.address)
        .collect())
}

fn selected_local_interfaces(
    requested: &[Ipv4Addr],
    allow_public: bool,
) -> Result<Vec<LocalIpv4Interface>> {
    let available = if_addrs::get_if_addrs()
        .context("enumerate network interfaces")?
        .into_iter()
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(address) if !address.ip.is_unspecified() => {
                Some(LocalIpv4Interface {
                    address: address.ip,
                    netmask: address.netmask,
                    prefix_length: address.prefixlen,
                    broadcast: address.broadcast,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let default_route = if requested.is_empty() {
        Some(default_route_ipv4()?)
    } else {
        None
    };
    select_interfaces(available, requested, allow_public, default_route)
}

fn default_route_ipv4() -> Result<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .context("bind socket for default IPv4 route probe")?;
    socket
        .connect((Ipv4Addr::new(1, 1, 1, 1), 9))
        .context("probe active default IPv4 route")?;
    match socket.local_addr()?.ip() {
        IpAddr::V4(address) if !address.is_unspecified() => Ok(address),
        address => bail!("default route probe returned unusable local address {address}"),
    }
}

fn select_interfaces(
    mut available: Vec<LocalIpv4Interface>,
    requested: &[Ipv4Addr],
    allow_public: bool,
    default_route: Option<Ipv4Addr>,
) -> Result<Vec<LocalIpv4Interface>> {
    available.sort_unstable();
    available.dedup();
    if !requested.is_empty() {
        for address in requested {
            if !available
                .iter()
                .any(|interface| interface.address == *address)
            {
                bail!("requested interface address {address} is not local");
            }
            if !allow_public && !is_private_or_local(*address) {
                bail!(
                    "requested interface address {address} is public; pass --allow-public-lan to opt in"
                );
            }
            let interface = available
                .iter()
                .find(|interface| interface.address == *address)
                .expect("requested interface was already found");
            if !address.is_loopback()
                && (interface.broadcast.is_none() || interface.prefix_length >= 32)
            {
                bail!(
                    "requested interface address {address} is not broadcast-capable with a prefix shorter than /32"
                );
            }
        }
        let mut selected: Vec<LocalIpv4Interface> = Vec::with_capacity(requested.len());
        for address in requested {
            if selected
                .iter()
                .any(|interface| interface.address == *address)
            {
                continue;
            }
            selected.push(
                available
                    .iter()
                    .find(|interface| interface.address == *address)
                    .copied()
                    .expect("requested interface was already validated"),
            );
        }
        return Ok(selected);
    }
    let default_route = default_route.context("no active default IPv4 route found")?;
    let interface = available
        .iter()
        .copied()
        .find(|interface| interface.address == default_route)
        .with_context(|| {
            format!(
                "active default-route IPv4 address {default_route} was not found among local interfaces"
            )
        })?;
    if interface.address.is_loopback()
        || interface.broadcast.is_none()
        || interface.prefix_length >= 32
    {
        bail!(
            "active default-route IPv4 address {default_route} is a tunnel or non-broadcast interface (prefix /{}); select a LAN address explicitly with --interface",
            interface.prefix_length
        );
    }
    if !allow_public && !is_private_lan(interface.address) {
        bail!(
            "active default-route IPv4 address {default_route} is public; pass --allow-public-lan to opt in or select a private LAN address with --interface"
        );
    }
    Ok(vec![interface])
}

#[must_use]
pub fn new_packet(
    session_id: String,
    contract: SessionContract,
    game_port: u16,
    now_ms: u64,
    ttl: Duration,
    reconnect_window: Duration,
) -> DiscoveryPacket {
    let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
    let reconnect_ms = u64::try_from(reconnect_window.as_millis()).unwrap_or(u64::MAX);
    DiscoveryPacket {
        magic: DISCOVERY_MAGIC.to_owned(),
        discovery_version: DISCOVERY_VERSION,
        session_id,
        sent_at_unix_ms: now_ms,
        expires_at_unix_ms: now_ms.saturating_add(ttl_ms),
        started_at_unix_ms: now_ms,
        reconnect_until_unix_ms: now_ms.saturating_add(reconnect_ms),
        game_port,
        contract,
    }
}

/// Parses and validates one bounded discovery datagram.
///
/// # Errors
/// Returns an error for oversized, malformed, foreign, invalid, or stale packets.
pub fn parse_packet(bytes: &[u8], now_ms: u64) -> Result<DiscoveryPacket> {
    if bytes.is_empty() || bytes.len() > MAX_PACKET_BYTES {
        bail!("invalid discovery packet size: {}", bytes.len());
    }
    let packet: DiscoveryPacket =
        serde_json::from_slice(bytes).context("parse discovery packet JSON")?;
    if packet.magic != DISCOVERY_MAGIC || packet.discovery_version != DISCOVERY_VERSION {
        bail!("unsupported discovery protocol");
    }
    if packet.session_id.is_empty() || packet.session_id.len() > 128 {
        bail!("invalid discovery session ID");
    }
    if packet.game_port == 0 {
        bail!("invalid game port 0");
    }
    if packet.sent_at_unix_ms > packet.expires_at_unix_ms
        || packet.started_at_unix_ms > packet.sent_at_unix_ms
        || packet.expires_at_unix_ms < now_ms
        || packet.sent_at_unix_ms > now_ms.saturating_add(MAX_CLOCK_SKEW_MS)
        || packet
            .expires_at_unix_ms
            .saturating_sub(packet.sent_at_unix_ms)
            > MAX_ADVERTISEMENT_TTL_MS
        || packet.reconnect_until_unix_ms < packet.started_at_unix_ms
    {
        bail!("stale or inconsistent discovery timestamps");
    }
    Ok(packet)
}

/// Receives same-subnet advertisements until the configured deadline.
///
/// # Errors
/// Returns an error when sockets or interface membership cannot be configured.
pub fn discover(options: &DiscoveryOptions) -> Result<Vec<DiscoveredSession>> {
    validate_discovery_options(options)?;
    let interfaces = selected_local_interfaces(&options.interfaces, options.allow_public)?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, options.discovery_port))
        .with_context(|| format!("bind UDP discovery port {}", options.discovery_port))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    for interface in &interfaces {
        socket
            .join_multicast_v4(&options.multicast_address, &interface.address)
            .with_context(|| format!("join multicast on {}", interface.address))?;
    }
    let deadline = std::time::Instant::now() + options.timeout;
    let mut buffer = vec![0_u8; MAX_PACKET_BYTES + 1].into_boxed_slice();
    let mut sessions: Vec<DiscoveredSession> = Vec::new();
    while std::time::Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                let IpAddr::V4(source_ip) = source.ip() else {
                    continue;
                };
                if !source_matches_selected_interfaces(source_ip, &interfaces) {
                    continue;
                }
                let Ok(packet) = parse_packet(&buffer[..length], now_unix_ms()) else {
                    continue;
                };
                let session = DiscoveredSession {
                    source,
                    session_id: packet.session_id,
                    started_at_unix_ms: packet.started_at_unix_ms,
                    expires_at_unix_ms: packet.expires_at_unix_ms,
                    reconnect_until_unix_ms: packet.reconnect_until_unix_ms,
                    game_port: packet.game_port,
                    contract: packet.contract,
                };
                if let Some(existing) = sessions
                    .iter_mut()
                    .find(|existing| existing.session_id == session.session_id)
                {
                    if discovered_session_preferred(&session, existing) {
                        *existing = session;
                    }
                } else {
                    sessions.push(session);
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(error).context("receive LAN discovery packet"),
        }
    }
    sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    Ok(sessions)
}

/// Sends one local-only advertisement through every selected interface.
///
/// # Errors
/// Returns an error when serialization, socket setup, or every send attempt fails.
pub fn advertise_once(packet: &DiscoveryPacket, options: &DiscoveryOptions) -> Result<()> {
    validate_discovery_options(options)?;
    let interfaces = selected_local_interfaces(&options.interfaces, options.allow_public)?;
    let payload = serde_json::to_vec(packet).context("serialize discovery packet")?;
    if payload.len() > MAX_PACKET_BYTES {
        bail!("discovery packet exceeds {MAX_PACKET_BYTES} bytes");
    }
    let mut successes = 0_usize;
    let mut last_error = None;
    for interface in interfaces {
        let socket = UdpSocket::bind((interface.address, 0))?;
        socket.set_broadcast(true)?;
        socket.set_multicast_ttl_v4(1)?;
        let mut targets = vec![SocketAddr::from((
            options.multicast_address,
            options.discovery_port,
        ))];
        if let Some(broadcast) = interface.broadcast {
            targets.push(SocketAddr::from((broadcast, options.discovery_port)));
        }
        for target in targets {
            match socket.send_to(&payload, target) {
                Ok(_) => successes += 1,
                Err(error) => last_error = Some(error),
            }
        }
    }
    if successes == 0 {
        return Err(anyhow!(
            last_error.unwrap_or_else(|| std::io::Error::other("no LAN interface"))
        ))
        .context("broadcast LAN discovery packet");
    }
    Ok(())
}

/// Builds the exact `RetroArch` role/address/port suffix.
///
/// # Errors
/// Returns an error for port zero or a client without a peer address.
pub fn retroarch_netplay_arguments(
    role: NetplayRole,
    peer_address: Option<&str>,
    port: u16,
) -> Result<Vec<String>> {
    if port == 0 {
        bail!("netplay port must not be zero");
    }
    let mut arguments = match role {
        NetplayRole::Host => vec!["--host".to_owned()],
        NetplayRole::Client => vec![
            "--connect".to_owned(),
            peer_address
                .filter(|address| !address.is_empty())
                .ok_or_else(|| anyhow!("client netplay role requires netplay.peer_address"))?
                .to_owned(),
        ],
    };
    arguments.extend(["--port".to_owned(), port.to_string()]);
    Ok(arguments)
}

fn validate_discovery_options(options: &DiscoveryOptions) -> Result<()> {
    if options.discovery_port == 0 {
        bail!("discovery port must not be zero");
    }
    if !options.multicast_address.is_multicast() || options.multicast_address.octets()[0] != 239 {
        bail!("multicast address must be in administratively scoped 239.0.0.0/8");
    }
    Ok(())
}

fn discovered_session_preferred(
    candidate: &DiscoveredSession,
    existing: &DiscoveredSession,
) -> bool {
    session_preference_key(candidate) > session_preference_key(existing)
}

fn session_preference_key(session: &DiscoveredSession) -> (bool, u64, u64, u64, SocketAddr) {
    (
        !session.source.ip().is_loopback(),
        session.started_at_unix_ms,
        session.expires_at_unix_ms,
        session.reconnect_until_unix_ms,
        session.source,
    )
}

/// Selects a compatible advertised session, optionally by reconnect session ID.
///
/// # Errors
/// Returns an error when no candidate has the requested game/ID and exact behavioral contract.
pub fn select_compatible<'a>(
    sessions: &'a [DiscoveredSession],
    local: &SessionContract,
    session_id: Option<&str>,
    now_ms: u64,
) -> Result<&'a DiscoveredSession> {
    let mut selected = None;
    let mut rejected = Vec::new();
    for session in sessions {
        if session_id.is_some_and(|requested| requested != session.session_id) {
            continue;
        }
        if session.expires_at_unix_ms < now_ms {
            rejected.push(format!("{}: advertisement expired", session.session_id));
            continue;
        }
        if session.reconnect_until_unix_ms < now_ms {
            rejected.push(format!("{}: reconnect window expired", session.session_id));
            continue;
        }
        let report = compare(local, &session.contract);
        if report.agreed {
            if selected.is_none_or(|current| discovered_session_preferred(session, current)) {
                selected = Some(session);
            }
            continue;
        }
        rejected.push(format!(
            "{}: {}",
            session.session_id,
            report
                .mismatches
                .iter()
                .map(|mismatch| mismatch.field.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(session) = selected {
        return Ok(session);
    }
    if rejected.is_empty() {
        if session_id.is_some() {
            bail!("no advertised LAN session matched the requested session ID");
        }
        bail!("no compatible LAN session found");
    }
    bail!("no compatible LAN session: {}", rejected.join("; "))
}

#[must_use]
pub fn compare(local: &SessionContract, peer: &SessionContract) -> AgreementReport {
    let mut mismatches = Vec::new();
    compare_field(
        &mut mismatches,
        "protocol_version",
        local.protocol_version.to_string(),
        peer.protocol_version.to_string(),
    );
    compare_field(
        &mut mismatches,
        "game",
        local.game.clone(),
        peer.game.clone(),
    );
    compare_field(
        &mut mismatches,
        "manifest_id",
        local.manifest_id.clone(),
        peer.manifest_id.clone(),
    );
    compare_field(
        &mut mismatches,
        "manifest_content_sha256",
        local.manifest_content_sha256.clone(),
        peer.manifest_content_sha256.clone(),
    );
    compare_field(
        &mut mismatches,
        "runtime_compatibility_id",
        local.runtime_compatibility_id.clone(),
        peer.runtime_compatibility_id.clone(),
    );
    compare_field(
        &mut mismatches,
        "core_compatibility_id",
        local
            .core_compatibility_id
            .as_deref()
            .unwrap_or("none")
            .to_owned(),
        peer.core_compatibility_id
            .as_deref()
            .unwrap_or("none")
            .to_owned(),
    );
    compare_field(
        &mut mismatches,
        "rom_sha256",
        local.rom_sha256.clone(),
        peer.rom_sha256.clone(),
    );
    compare_field(
        &mut mismatches,
        "bios_sha256",
        local.bios_sha256.clone(),
        peer.bios_sha256.clone(),
    );
    compare_field(
        &mut mismatches,
        "language",
        local.language.clone(),
        peer.language.clone(),
    );
    compare_field(
        &mut mismatches,
        "region",
        local.region.clone(),
        peer.region.clone(),
    );
    compare_field(
        &mut mismatches,
        "behavior_sha256",
        local.behavior_sha256.clone(),
        peer.behavior_sha256.clone(),
    );
    AgreementReport {
        agreed: mismatches.is_empty(),
        mismatches,
    }
}

/// Enforces byte-for-byte agreement of every prelaunch contract field.
///
/// # Errors
/// Returns an error listing every field whose value differs.
pub fn require_agreement(local: &SessionContract, peer: &SessionContract) -> Result<()> {
    let report = compare(local, peer);
    if report.agreed {
        return Ok(());
    }
    let fields = report
        .mismatches
        .iter()
        .map(|mismatch| mismatch.field.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("LAN prelaunch hash agreement failed: {fields}")
}

/// Loads a JSON session contract.
///
/// # Errors
/// Returns an error when the file cannot be read or parsed.
pub fn load(path: &Path) -> Result<SessionContract> {
    let bytes = fs::read(path).with_context(|| format!("read LAN contract {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse LAN contract {}", path.display()))
}

fn compare_field(mismatches: &mut Vec<ContractMismatch>, field: &str, local: String, peer: String) {
    if local != peer {
        mismatches.push(ContractMismatch {
            field: field.to_owned(),
            local,
            peer,
        });
    }
}

fn source_matches_selected_interfaces(source: Ipv4Addr, interfaces: &[LocalIpv4Interface]) -> bool {
    interfaces
        .iter()
        .copied()
        .any(|interface| source_matches_interface(source, interface))
}

fn source_matches_interface(source: Ipv4Addr, interface: LocalIpv4Interface) -> bool {
    if interface.address.is_loopback() {
        return source.is_loopback();
    }
    same_subnet(source, interface.address, interface.netmask)
}

fn same_subnet(left: Ipv4Addr, right: Ipv4Addr, netmask: Ipv4Addr) -> bool {
    let mask = u32::from(netmask);
    if mask == 0 {
        return left == right;
    }
    (u32::from(left) & mask) == (u32::from(right) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> SessionContract {
        SessionContract {
            protocol_version: SESSION_PROTOCOL_VERSION,
            game: "3".to_owned(),
            manifest_id: "mslug3-modern".to_owned(),
            manifest_content_sha256: "manifest-content".to_owned(),
            runtime_compatibility_id: "retroarch-netplay-v1".to_owned(),
            core_compatibility_id: Some("fbneo-custom-v1".to_owned()),
            rom_sha256: "rom".to_owned(),
            bios_sha256: "bios".to_owned(),
            language: "en".to_owned(),
            region: "europe".to_owned(),
            behavior_sha256: "behavior".to_owned(),
        }
    }

    #[test]
    fn exact_contracts_agree() {
        let local = contract();
        assert!(compare(&local, &local).agreed);
        assert!(require_agreement(&local, &local).is_ok());
    }

    #[test]
    fn any_hash_mismatch_blocks_prelaunch() {
        let local = contract();
        let mut peer = contract();
        peer.bios_sha256 = "different".to_owned();
        let report = compare(&local, &peer);
        assert!(!report.agreed);
        assert_eq!(report.mismatches[0].field, "bios_sha256");
        assert!(require_agreement(&local, &peer).is_err());
    }

    #[test]
    fn language_and_region_mismatches_block_prelaunch_explicitly() {
        let local = contract();
        let mut peer = contract();
        peer.language = "fr".to_owned();
        peer.region = "usa".to_owned();
        let fields = compare(&local, &peer)
            .mismatches
            .into_iter()
            .map(|mismatch| mismatch.field)
            .collect::<Vec<_>>();
        assert_eq!(fields, ["language", "region"]);
    }

    #[test]
    fn protocol_v2_cannot_agree_with_protocol_v3() {
        let local = contract();
        let mut peer = contract();
        peer.protocol_version = 2;
        let report = compare(&local, &peer);
        assert_eq!(report.mismatches.len(), 1);
        assert_eq!(report.mismatches[0].field, "protocol_version");
    }

    #[test]
    fn reports_all_mismatches_in_stable_field_order() {
        let local = contract();
        let mut peer = contract();
        peer.core_compatibility_id = Some("other-core".to_owned());
        peer.rom_sha256 = "other-rom".to_owned();
        peer.behavior_sha256 = "other-behavior".to_owned();
        let fields = compare(&local, &peer)
            .mismatches
            .into_iter()
            .map(|mismatch| mismatch.field)
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            ["core_compatibility_id", "rom_sha256", "behavior_sha256"]
        );
    }

    fn packet(now: u64) -> DiscoveryPacket {
        new_packet(
            "session-1".to_owned(),
            contract(),
            DEFAULT_GAME_PORT,
            now,
            Duration::from_secs(2),
            Duration::from_secs(120),
        )
    }

    fn discovered(now: u64, peer_contract: SessionContract) -> DiscoveredSession {
        DiscoveredSession {
            source: "192.168.1.20:40000".parse().unwrap(),
            session_id: "session-1".to_owned(),
            started_at_unix_ms: now,
            expires_at_unix_ms: now + 2_000,
            reconnect_until_unix_ms: now + 120_000,
            game_port: DEFAULT_GAME_PORT,
            contract: peer_contract,
        }
    }

    #[test]
    fn opposite_retroarch_roles_agree_and_have_exact_arguments() {
        let agreement = compare(&contract(), &contract());
        assert!(agreement.agreed);
        assert_eq!(
            retroarch_netplay_arguments(NetplayRole::Host, None, 55_435).unwrap(),
            ["--host", "--port", "55435"]
        );
        assert_eq!(
            retroarch_netplay_arguments(NetplayRole::Client, Some("192.168.1.20"), 55_435).unwrap(),
            ["--connect", "192.168.1.20", "--port", "55435"]
        );
    }

    #[test]
    fn incompatible_peer_is_rejected_before_launch() {
        let now = 1_000_000;
        let mut peer = contract();
        peer.rom_sha256 = "wrong-rom".to_owned();
        let error = select_compatible(&[discovered(now, peer)], &contract(), None, now)
            .unwrap_err()
            .to_string();
        assert!(error.contains("rom_sha256"));
    }

    #[test]
    fn advertisement_contains_no_machine_paths() {
        let encoded = serde_json::to_string(&packet(1_000_000)).unwrap();
        assert!(!encoded.contains("/home/"));
        assert!(!encoded.contains("C:\\"));
        assert!(!encoded.contains("path"));
        assert!(!encoded.contains("executable"));
    }

    #[test]
    fn packet_parser_rejects_malformed_unknown_and_oversized_input() {
        assert!(parse_packet(b"not-json", 1_000_000).is_err());
        assert!(parse_packet(&vec![b'x'; MAX_PACKET_BYTES + 1], 1_000_000).is_err());
        let mut value = serde_json::to_value(packet(1_000_000)).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(parse_packet(&serde_json::to_vec(&value).unwrap(), 1_000_000).is_err());
        let mut nested_unknown = serde_json::to_value(packet(1_000_000)).unwrap();
        nested_unknown["contract"]["unexpected"] = serde_json::json!(true);
        assert!(parse_packet(&serde_json::to_vec(&nested_unknown).unwrap(), 1_000_000).is_err());
        let mut missing_field = serde_json::to_value(packet(1_000_000)).unwrap();
        missing_field
            .as_object_mut()
            .unwrap()
            .remove("reconnect_until_unix_ms");
        assert!(parse_packet(&serde_json::to_vec(&missing_field).unwrap(), 1_000_000).is_err());
    }

    #[test]
    fn stale_advertisements_and_expired_reconnects_are_filtered() {
        let encoded = serde_json::to_vec(&packet(1_000_000)).unwrap();
        assert!(parse_packet(&encoded, 1_001_999).is_ok());
        assert!(parse_packet(&encoded, 1_002_001).is_err());

        let mut expired_advertisement = discovered(1_000_000, contract());
        expired_advertisement.expires_at_unix_ms = 1_000_100;
        let error = select_compatible(&[expired_advertisement], &contract(), None, 1_000_101)
            .unwrap_err()
            .to_string();
        assert!(error.contains("advertisement expired"));

        let mut session = discovered(1_000_000, contract());
        session.reconnect_until_unix_ms = 1_000_100;
        assert!(select_compatible(&[session], &contract(), None, 1_000_101).is_err());
    }

    #[test]
    fn private_address_policy_is_default() {
        assert!(is_private_or_local("10.1.2.3".parse().unwrap()));
        assert!(is_private_or_local("169.254.10.20".parse().unwrap()));
        assert!(is_private_or_local("192.168.1.2".parse().unwrap()));
        assert!(is_private_or_local("127.0.0.1".parse().unwrap()));
        assert!(!is_private_or_local("8.8.8.8".parse().unwrap()));
    }

    fn interface(
        address: &str,
        netmask: &str,
        prefix_length: u8,
        broadcast: Option<&str>,
    ) -> LocalIpv4Interface {
        LocalIpv4Interface {
            address: address.parse().unwrap(),
            netmask: netmask.parse().unwrap(),
            prefix_length,
            broadcast: broadcast.map(|value| value.parse().unwrap()),
        }
    }

    #[test]
    fn automatic_selection_uses_only_default_route_without_private_bridge_fallback() {
        let lan = interface("192.168.1.10", "255.255.255.0", 24, Some("192.168.1.255"));
        let inactive_bridge = interface("172.17.0.1", "255.255.0.0", 16, Some("172.17.255.255"));
        assert_eq!(
            select_interfaces(vec![inactive_bridge, lan], &[], false, Some(lan.address)).unwrap(),
            vec![lan]
        );

        let error = select_interfaces(
            vec![inactive_bridge],
            &[],
            false,
            Some("192.168.1.10".parse().unwrap()),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("not found among local interfaces"));
    }

    #[test]
    fn automatic_32_bit_tunnel_is_rejected_actionably() {
        let tunnel = interface("100.64.0.2", "255.255.255.255", 32, None);
        let error = select_interfaces(vec![tunnel], &[], false, Some(tunnel.address))
            .unwrap_err()
            .to_string();
        assert!(error.contains("tunnel or non-broadcast interface"));
        assert!(error.contains("--interface"));
    }

    #[test]
    fn public_default_route_requires_explicit_opt_in() {
        let public = interface("8.8.8.8", "255.255.255.0", 24, Some("8.8.8.255"));
        let error = select_interfaces(vec![public], &[], false, Some(public.address))
            .unwrap_err()
            .to_string();
        assert!(error.contains("--allow-public-lan"));
        assert_eq!(
            select_interfaces(vec![public], &[], true, Some(public.address)).unwrap(),
            vec![public]
        );
    }

    #[test]
    fn explicit_selection_preserves_multiple_interfaces_and_loopback() {
        let loopback = interface("127.0.0.1", "255.0.0.0", 8, None);
        let lan = interface("192.168.1.10", "255.255.255.0", 24, Some("192.168.1.255"));
        assert_eq!(
            select_interfaces(
                vec![lan, loopback],
                &[loopback.address, lan.address],
                false,
                None,
            )
            .unwrap(),
            vec![loopback, lan]
        );
    }

    #[test]
    fn explicit_nonexistent_address_is_rejected() {
        let error = select_interfaces(
            vec![interface(
                "192.168.1.10",
                "255.255.255.0",
                24,
                Some("192.168.1.255"),
            )],
            &["192.168.1.99".parse().unwrap()],
            false,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("is not local"));
    }

    #[test]
    fn explicit_non_loopback_must_be_broadcast_capable() {
        let tunnel = interface("10.0.0.2", "255.255.255.255", 32, None);
        let error = select_interfaces(vec![tunnel], &[tunnel.address], false, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not broadcast-capable"));
    }

    #[test]
    fn real_22_bit_netmask_controls_source_subnet() {
        let lan = interface("192.168.4.10", "255.255.252.0", 22, Some("192.168.7.255"));
        assert!(source_matches_interface(
            "192.168.7.200".parse().unwrap(),
            lan
        ));
        assert!(!source_matches_interface(
            "192.168.8.1".parse().unwrap(),
            lan
        ));
    }

    #[test]
    fn discovery_sources_must_match_selected_interface_subnets() {
        let interfaces = [
            interface("192.168.1.10", "255.255.255.0", 24, Some("192.168.1.255")),
            interface("169.254.20.30", "255.255.0.0", 16, Some("169.254.255.255")),
        ];
        assert!(source_matches_selected_interfaces(
            "192.168.1.200".parse().unwrap(),
            &interfaces,
        ));
        assert!(source_matches_selected_interfaces(
            "169.254.99.40".parse().unwrap(),
            &interfaces,
        ));
        assert!(!source_matches_selected_interfaces(
            "10.10.10.10".parse().unwrap(),
            &interfaces,
        ));
        assert!(!source_matches_selected_interfaces(
            "192.168.2.20".parse().unwrap(),
            &interfaces,
        ));
    }

    #[test]
    fn explicit_loopback_selection_preserves_loopback_discovery() {
        let interfaces = [
            interface("127.0.0.1", "255.0.0.0", 8, None),
            interface("192.168.1.10", "255.255.255.0", 24, Some("192.168.1.255")),
        ];
        assert!(source_matches_selected_interfaces(
            "127.0.0.1".parse().unwrap(),
            &interfaces,
        ));
        assert!(source_matches_selected_interfaces(
            "192.168.1.20".parse().unwrap(),
            &interfaces,
        ));
    }

    #[test]
    fn compatible_session_selection_prefers_newest_non_loopback_session() {
        let now = 1_000_000;
        let older = DiscoveredSession {
            source: "127.0.0.1:40000".parse().unwrap(),
            session_id: "older".to_owned(),
            started_at_unix_ms: now,
            expires_at_unix_ms: now + 1_000,
            reconnect_until_unix_ms: now + 90_000,
            game_port: DEFAULT_GAME_PORT,
            contract: contract(),
        };
        let newer = DiscoveredSession {
            source: "192.168.1.20:40000".parse().unwrap(),
            session_id: "newer".to_owned(),
            started_at_unix_ms: now + 5_000,
            expires_at_unix_ms: now + 6_000,
            reconnect_until_unix_ms: now + 120_000,
            game_port: DEFAULT_GAME_PORT,
            contract: contract(),
        };
        let sessions = [older, newer];
        let selected = select_compatible(&sessions, &contract(), None, now).unwrap();
        assert_eq!(selected.session_id, "newer");
        assert_eq!(selected.source.ip().to_string(), "192.168.1.20");
    }
}
