//! Hand-rolled connector: resolve → TCP connect → (proxy tunnel) → rustls
//! handshake with ALPN, recording when each phase ends so the caller can
//! report DNS / Connect / TLS timings.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::super::types::{HttpVersionPref, SendError};

/// Any byte stream the HTTP layer can run over (TCP, TLS, TLS-in-TLS).
pub(super) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub(super) type BoxIo = Box<dyn Io>;

/// Where a hop goes.
pub(super) struct Target {
    pub https: bool,
    /// Host without IPv6 brackets (for DNS and SNI).
    pub host: String,
    pub port: u16,
}

/// A forward proxy from `settings.proxy`.
#[derive(Clone, Debug)]
pub(super) struct Proxy {
    /// `https://` proxy: TLS to the proxy itself.
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Ready-made `Proxy-Authorization` value.
    pub auth: Option<String>,
}

/// Instants recorded while connecting.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Phases {
    pub dns_start: Option<Instant>,
    pub dns_end: Option<Instant>,
    pub tcp_end: Option<Instant>,
    pub tls_end: Option<Instant>,
    pub addr: Option<SocketAddr>,
}

pub(super) struct Connected {
    pub io: BoxIo,
    /// ALPN negotiated `h2`.
    pub h2: bool,
}

fn net(code: &str, message: String, host: &str) -> SendError {
    SendError::Network {
        code: code.into(),
        message,
        host: host.into(),
    }
}

/// Error code for an I/O failure (`TLS` when rustls is behind it).
pub(super) fn io_code(e: &io::Error) -> &'static str {
    use io::ErrorKind as K;
    if e.get_ref()
        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some())
    {
        return "TLS";
    }
    match e.kind() {
        K::ConnectionRefused => "ECONNREFUSED",
        K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe | K::UnexpectedEof => {
            "ECONNRESET"
        }
        K::TimedOut => "ETIMEDOUT",
        K::NotFound | K::AddrNotAvailable => "ENOTFOUND",
        _ => "EIO",
    }
}

/// `host[:port]` / `[v6]:port` → (host without brackets, port).
pub(super) fn split_host_port(hostport: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) if !p.is_empty() => p.parse().ok()?,
            _ => default_port,
        };
        return Some((host.to_string(), port));
    }
    match hostport.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => Some((host.to_string(), port.parse().ok()?)),
        Some((host, _)) => Some((host.to_string(), default_port)),
        None => Some((hostport.to_string(), default_port)),
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Parses `http://[user:pass@]host[:port]` (scheme optional, defaults to
/// http) or `https://…` (TLS to the proxy).
pub(super) fn parse_proxy(raw: &str) -> Result<Proxy, String> {
    let raw = raw.trim();
    let (scheme, rest) = raw.split_once("://").unwrap_or(("http", raw));
    let tls = match scheme.to_ascii_lowercase().as_str() {
        "http" => false,
        "https" => true,
        other => {
            return Err(format!(
                "Unsupported proxy scheme {other}:// (only http:// and https:// proxies are supported)"
            ));
        }
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let (host, port) = split_host_port(hostport, if tls { 443 } else { 80 })
        .filter(|(h, _)| !h.is_empty())
        .ok_or_else(|| format!("Invalid proxy URL: {raw}"))?;
    let auth = userinfo.map(|u| {
        let (user, pass) = u.split_once(':').unwrap_or((u, ""));
        let mut creds = percent_decode(user);
        creds.push(b':');
        creds.extend(percent_decode(pass));
        format!("Basic {}", super::super::vars::base64(&creds))
    });
    Ok(Proxy {
        tls,
        host,
        port,
        auth,
    })
}

fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

fn roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            Arc::new(RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
        })
        .clone()
}

/// `ssl_verify = false`: accepts any certificate and signature.
#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config(verify: bool, alpn: &[&[u8]]) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = provider();
    let builder =
        ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions()?;
    let mut config = if verify {
        builder.with_root_certificates(roots()).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
            .with_no_client_auth()
    };
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(config))
}

async fn tls_handshake(
    io: BoxIo,
    server: &str,
    verify: bool,
    alpn: &[&[u8]],
    label: &str,
) -> Result<(BoxIo, bool), SendError> {
    let config = tls_config(verify, alpn).map_err(|e| net("TLS", e.to_string(), label))?;
    let name = ServerName::try_from(server.to_string())
        .map_err(|e| net("TLS", format!("Invalid TLS server name {server}: {e}"), label))?;
    let stream = tokio_rustls::TlsConnector::from(config)
        .connect(name, io)
        .await
        .map_err(|e| {
            let code = match io_code(&e) {
                c @ ("ECONNRESET" | "ETIMEDOUT" | "TLS") => c,
                _ => "TLS",
            };
            net(code, e.to_string(), label)
        })?;
    let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
    Ok((Box::new(stream), h2))
}

/// `CONNECT host:port` through an established proxy connection.
async fn tunnel(
    io: &mut BoxIo,
    target: &Target,
    proxy: &Proxy,
    label: &str,
) -> Result<(), SendError> {
    let authority = if target.host.contains(':') {
        format!("[{}]:{}", target.host, target.port)
    } else {
        format!("{}:{}", target.host, target.port)
    };
    let mut head = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = &proxy.auth {
        head.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    head.push_str("\r\n");
    let io_err = |e: io::Error| net(io_code(&e), format!("Proxy: {e}"), label);
    io.write_all(head.as_bytes()).await.map_err(io_err)?;
    io.flush().await.map_err(io_err)?;
    // Read the reply head byte by byte so nothing past it is consumed.
    let mut reply = Vec::new();
    while !reply.ends_with(b"\r\n\r\n") {
        if reply.len() > 64 * 1024 {
            return Err(net("EIO", "Proxy reply too large".into(), label));
        }
        let b = io.read_u8().await.map_err(io_err)?;
        reply.push(b);
    }
    let text = String::from_utf8_lossy(&reply);
    let status_line = text.lines().next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    match status {
        200..=299 => Ok(()),
        407 => Err(net(
            "EIO",
            "Proxy authentication required (407)".into(),
            label,
        )),
        _ => Err(net(
            "EIO",
            format!("Proxy CONNECT failed: {}", status_line.trim()),
            label,
        )),
    }
}

/// ALPN list for a TLS connection to the origin.
fn alpn_for(pref: HttpVersionPref) -> &'static [&'static [u8]] {
    match pref {
        HttpVersionPref::Auto => &[b"h2", b"http/1.1"],
        HttpVersionPref::Http1 => &[b"http/1.1"],
        HttpVersionPref::Http2 => &[b"h2"],
    }
}

/// Opens a connection to `target` (via `proxy` if any), filling `phases`.
/// `label` is the host shown in errors.
pub(super) async fn connect(
    target: &Target,
    proxy: Option<&Proxy>,
    verify: bool,
    pref: HttpVersionPref,
    phases: &mut Phases,
    label: &str,
) -> Result<Connected, SendError> {
    let (dial_host, dial_port) = match proxy {
        Some(p) => (p.host.as_str(), p.port),
        None => (target.host.as_str(), target.port),
    };

    phases.dns_start = Some(Instant::now());
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((dial_host, dial_port))
        .await
        .map_err(|e| {
            net(
                "ENOTFOUND",
                format!("Could not resolve host {dial_host}: {e}"),
                label,
            )
        })?
        .collect();
    phases.dns_end = Some(Instant::now());
    if addrs.is_empty() {
        return Err(net(
            "ENOTFOUND",
            format!("Could not resolve host {dial_host}"),
            label,
        ));
    }

    let mut last_err = None;
    let mut tcp = None;
    for addr in &addrs {
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                phases.addr = Some(*addr);
                tcp = Some(stream);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let Some(tcp) = tcp else {
        let e = last_err.unwrap_or_else(|| io::Error::other("no address"));
        let code = match io_code(&e) {
            "TLS" | "ENOTFOUND" => "ECONNREFUSED",
            c => c,
        };
        return Err(net(code, format!("Could not connect to {label}: {e}"), label));
    };
    let mut io: BoxIo = Box::new(tcp);

    if let Some(p) = proxy {
        if p.tls {
            io = tls_handshake(io, &p.host, verify, &[b"http/1.1"], label).await?.0;
        }
        if target.https {
            tunnel(&mut io, target, p, label).await?;
        }
    }
    phases.tcp_end = Some(Instant::now());

    if !target.https {
        return Ok(Connected { io, h2: false });
    }
    let (io, h2) = tls_handshake(io, &target.host, verify, alpn_for(pref), label).await?;
    phases.tls_end = Some(Instant::now());
    if pref == HttpVersionPref::Http2 && !h2 {
        return Err(net(
            "EIO",
            format!("{label} did not negotiate HTTP/2 (ALPN h2)"),
            label,
        ));
    }
    Ok(Connected { io, h2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_urls() {
        let p = parse_proxy("http://u%40x:p%3A@proxy.local:3128").unwrap();
        assert!(!p.tls);
        assert_eq!((p.host.as_str(), p.port), ("proxy.local", 3128));
        assert_eq!(
            p.auth.as_deref(),
            Some(format!("Basic {}", super::super::super::vars::base64(b"u@x:p:")).as_str())
        );
        let p = parse_proxy("proxy.local").unwrap();
        assert_eq!((p.port, p.tls, p.auth.is_none()), (80, false, true));
        let p = parse_proxy("https://[::1]:8443/").unwrap();
        assert_eq!((p.host.as_str(), p.port, p.tls), ("::1", 8443, true));
        assert!(parse_proxy("socks5://h:1080").is_err());
    }

    #[test]
    fn host_ports() {
        assert_eq!(split_host_port("a.b", 80), Some(("a.b".into(), 80)));
        assert_eq!(split_host_port("a.b:81", 80), Some(("a.b".into(), 81)));
        assert_eq!(split_host_port("[::1]", 443), Some(("::1".into(), 443)));
        assert_eq!(split_host_port("[::1]:9", 443), Some(("::1".into(), 9)));
        assert_eq!(split_host_port("a:x", 80), None);
    }
}
