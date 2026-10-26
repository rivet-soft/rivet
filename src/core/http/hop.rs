//! One request/response exchange over a fresh connection (HTTP/1.1 or
//! HTTP/2 via hyper), plus response decoding and error mapping.

use std::error::Error as _;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cookie_store::{CookieStore, RawCookie};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::{http1, http2};
use hyper_util::rt::{TokioExecutor, TokioIo};

use super::super::types::{HttpVersionPref, Method, SendError, Settings, Timings};
use super::conn::{self, Phases, Proxy, Target, io_code, split_host_port};
use super::{MAX_BODY_BYTES, header_value, host_of, is_redirect, status_text};

pub(super) struct HopRequest<'a> {
    pub method: Method,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    /// Client default headers not to add (lowercase names).
    pub omit_defaults: &'a [String],
    /// Reports body bytes read so far and the `Content-Length` (0 if unknown).
    pub progress: Option<&'a super::Progress>,
}

pub(super) struct HopResponse {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub headers: Vec<(String, String)>,
    pub remote_addr: Option<String>,
    pub body: Vec<u8>,
    /// `redirect` is zero; `total` is this hop's elapsed time.
    pub timings: Timings,
}

/// Aborts the connection driver task when the hop ends (or is cancelled).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn method_of(m: Method) -> http::Method {
    match m {
        Method::Get => http::Method::GET,
        Method::Post => http::Method::POST,
        Method::Put => http::Method::PUT,
        Method::Patch => http::Method::PATCH,
        Method::Delete => http::Method::DELETE,
        Method::Head => http::Method::HEAD,
        Method::Options => http::Method::OPTIONS,
    }
}

fn version_str(v: http::Version) -> &'static str {
    match v {
        http::Version::HTTP_09 => "HTTP/0.9",
        http::Version::HTTP_10 => "HTTP/1.0",
        http::Version::HTTP_2 => "HTTP/2",
        http::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/1.1",
    }
}

/// Maps a hyper error (request write, head or body read) to a [`SendError`].
fn hyper_error(e: &hyper::Error, host: &str) -> SendError {
    let mut message = e.to_string();
    let mut code = None;
    let mut source = e.source();
    while let Some(s) = source {
        message.push_str(": ");
        message.push_str(&s.to_string());
        if code.is_none()
            && let Some(io) = s.downcast_ref::<std::io::Error>()
        {
            code = Some(io_code(io));
        }
        source = s.source();
    }
    let code = code.unwrap_or(if e.is_incomplete_message() || e.is_closed() || e.is_canceled() {
        "ECONNRESET"
    } else {
        "EIO"
    });
    SendError::Network {
        code: code.into(),
        message,
        host: host.into(),
    }
}

/// Undoes `Content-Encoding` (gzip / deflate / br, possibly stacked).
/// `None` when the encoding is unknown or the data does not decode.
fn decode_body(raw: &[u8], encoding: &str) -> Option<Vec<u8>> {
    let codings: Vec<String> = encoding
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "identity")
        .collect();
    if codings.is_empty() {
        return None;
    }
    let limit = MAX_BODY_BYTES as u64;
    let mut data = raw.to_vec();
    for coding in codings.iter().rev() {
        let mut out = Vec::new();
        let ok = match coding.as_str() {
            "gzip" | "x-gzip" => flate2::read::MultiGzDecoder::new(&data[..])
                .take(limit)
                .read_to_end(&mut out)
                .is_ok(),
            // Servers disagree on zlib-wrapped vs raw deflate; try both.
            "deflate" => {
                flate2::read::ZlibDecoder::new(&data[..])
                    .take(limit)
                    .read_to_end(&mut out)
                    .is_ok()
                    || {
                        out.clear();
                        flate2::read::DeflateDecoder::new(&data[..])
                            .take(limit)
                            .read_to_end(&mut out)
                            .is_ok()
                    }
            }
            "br" => brotli_decompressor::Decompressor::new(&data[..], 64 * 1024)
                .take(limit)
                .read_to_end(&mut out)
                .is_ok(),
            _ => false,
        };
        if !ok {
            return None;
        }
        data = out;
    }
    Some(data)
}

/// Sends exactly one request over a new connection. The body of a response
/// that the caller will follow (`skip_redirect_body`) is not read.
pub(super) async fn run_hop(
    hop: &HopRequest<'_>,
    settings: &Settings,
    proxy: Option<&Proxy>,
    jar: &Mutex<CookieStore>,
    skip_redirect_body: bool,
) -> Result<HopResponse, SendError> {
    let label = host_of(hop.url);
    let start = Instant::now();
    let invalid = |m: String| SendError::InvalidUrl(m);

    let raw_url = hop.url.split('#').next().unwrap_or("");
    let uri: http::Uri = raw_url
        .parse()
        .map_err(|e| invalid(format!("{raw_url}: {e}")))?;
    let https = match uri.scheme_str() {
        Some(s) if s.eq_ignore_ascii_case("https") => true,
        Some(s) if s.eq_ignore_ascii_case("http") => false,
        Some(s) => return Err(invalid(format!("Unsupported URL scheme {s}://"))),
        None => return Err(invalid(format!("{raw_url}: missing scheme"))),
    };
    let authority = uri
        .authority()
        .ok_or_else(|| invalid(format!("{raw_url}: missing host")))?;
    let host_port = authority
        .as_str()
        .rsplit_once('@')
        .map_or(authority.as_str(), |(_, h)| h)
        .to_string();
    let (host, port) = split_host_port(&host_port, if https { 443 } else { 80 })
        .filter(|(h, _)| !h.is_empty())
        .ok_or_else(|| invalid(format!("{raw_url}: invalid host")))?;
    let path_query = uri
        .path_and_query()
        .map(|p| p.as_str())
        .filter(|p| !p.is_empty())
        .unwrap_or("/")
        .to_string();
    let cookie_url = url::Url::parse(raw_url).ok();

    // Headers actually sent: the caller's, plus cookies and defaults.
    let mut headers: Vec<(String, String)> = hop.headers.to_vec();
    if let Some(cookie_url) = &cookie_url {
        let jar_value = jar
            .lock()
            .map(|jar| {
                jar.get_request_values(cookie_url)
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        if !jar_value.is_empty() {
            match headers
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case("cookie"))
            {
                Some((_, v)) if !v.trim().is_empty() => *v = format!("{v}; {jar_value}"),
                Some((_, v)) => *v = jar_value,
                None => headers.push(("cookie".into(), jar_value)),
            }
        }
    }
    for (name, value) in crate::core::auto_headers::CLIENT_DEFAULTS {
        if header_value(&headers, name).is_none() && !hop.omit_defaults.iter().any(|o| o == name) {
            headers.push((name.into(), value.into()));
        }
    }

    let pref = settings.http_version;
    let target = Target {
        https,
        host,
        port,
    };
    let mut phases = Phases::default();
    let connected = conn::connect(
        &target,
        proxy,
        settings.ssl_verify,
        pref,
        &mut phases,
        &label,
    )
    .await?;
    let use_h2 = if https {
        connected.h2
    } else {
        proxy.is_none() && pref == HttpVersionPref::Http2
    };
    let absolute = use_h2 || (proxy.is_some() && !https);

    let request_uri = if absolute {
        format!(
            "{}://{host_port}{path_query}",
            if https { "https" } else { "http" }
        )
    } else {
        path_query
    };
    let mut builder = http::Request::builder()
        .method(method_of(hop.method))
        .uri(request_uri)
        .version(if use_h2 {
            http::Version::HTTP_2
        } else {
            http::Version::HTTP_11
        });
    if !use_h2 && header_value(&headers, "host").is_none() {
        builder = builder.header(http::header::HOST, host_port.as_str());
    }
    if !https
        && let Some(auth) = proxy.and_then(|p| p.auth.as_deref())
        && header_value(&headers, "proxy-authorization").is_none()
    {
        builder = builder.header(http::header::PROXY_AUTHORIZATION, auth);
    }
    for (k, v) in &headers {
        // `from_bytes` also takes UTF-8 (obs-text); `vars::resolve` already
        // rejected line breaks and control characters with a readable error.
        builder = builder.header(k.as_str(), http::HeaderValue::from_bytes(v.as_bytes()).map_err(|e| invalid(format!("{k}: {e}")))?);
    }
    let payload = match hop.body {
        Some(body) if hop.method != Method::Head => Bytes::copy_from_slice(body),
        _ => Bytes::new(),
    };
    let request = builder
        .body(Full::new(payload))
        .map_err(|e| invalid(e.to_string()))?;

    let io = TokioIo::new(connected.io);
    let err = |e: hyper::Error| hyper_error(&e, &label);
    let (response, _driver) = if use_h2 {
        let (mut sender, connection) = http2::handshake(TokioExecutor::new(), io)
            .await
            .map_err(err)?;
        let driver = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));
        (sender.send_request(request).await.map_err(err)?, driver)
    } else {
        let (mut sender, connection) = http1::Builder::new()
            .allow_obsolete_multiline_headers_in_responses(true)
            .allow_spaces_after_header_name_in_responses(true)
            .handshake(io)
            .await
            .map_err(err)?;
        let driver = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));
        (sender.send_request(request).await.map_err(err)?, driver)
    };
    let head_at = Instant::now();

    let (parts, mut incoming) = response.into_parts();
    let status = parts.status.as_u16();
    let http_version = version_str(parts.version).to_string();
    let mut headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    if let Some(cookie_url) = &cookie_url
        && let Ok(mut jar) = jar.lock()
    {
        let cookies = parts
            .headers
            .get_all(http::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|s| RawCookie::parse(s.to_string()).ok());
        jar.store_response_cookies(cookies, cookie_url);
    }
    let followed =
        skip_redirect_body && is_redirect(status) && header_value(&headers, "location").is_some();

    let mut body = Vec::new();
    if hop.method != Method::Head && !followed {
        let expected = header_value(&headers, "content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);
        while let Some(frame) = incoming.frame().await {
            let frame = frame.map_err(err)?;
            if let Ok(data) = frame.into_data() {
                let take = data.len().min(MAX_BODY_BYTES - body.len());
                body.extend_from_slice(&data[..take]);
                if let Some(progress) = hop.progress {
                    progress(body.len() as u64, expected);
                }
                if body.len() >= MAX_BODY_BYTES {
                    break;
                }
            }
        }
    }
    if !body.is_empty()
        && let Some(decoded) =
            header_value(&headers, "content-encoding").and_then(|enc| decode_body(&body, enc))
    {
        body = decoded;
        // The headers now describe the decoded body.
        headers.retain(|(k, _)| {
            !k.eq_ignore_ascii_case("content-encoding") && !k.eq_ignore_ascii_case("content-length")
        });
    }
    let end = Instant::now();

    let span = |a: Option<Instant>, b: Option<Instant>| match (a, b) {
        (Some(a), Some(b)) => b.saturating_duration_since(a),
        _ => Duration::ZERO,
    };
    let ready = phases.tls_end.or(phases.tcp_end).unwrap_or(start);
    let timings = Timings {
        redirect: Duration::ZERO,
        dns: span(phases.dns_start, phases.dns_end),
        connect: span(phases.dns_end, phases.tcp_end),
        tls: span(phases.tcp_end, phases.tls_end),
        ttfb: head_at.saturating_duration_since(ready),
        download: end.saturating_duration_since(head_at),
        total: end.saturating_duration_since(start),
    };
    let reason = status_text(status);
    let status_text = if reason.is_empty() {
        parts.status.canonical_reason().unwrap_or("").to_string()
    } else {
        reason.to_string()
    };
    let remote_addr = if proxy.is_some() {
        None
    } else {
        phases.addr.map(|a| a.to_string())
    };

    Ok(HopResponse {
        status,
        status_text,
        http_version,
        headers,
        remote_addr,
        body,
        timings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn decodes_content_encodings() {
        let text = b"hello hello hello";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(text).unwrap();
        let gz = gz.finish().unwrap();
        assert_eq!(decode_body(&gz, "gzip").unwrap(), text);

        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(text).unwrap();
        assert_eq!(decode_body(&z.finish().unwrap(), "deflate").unwrap(), text);

        let mut d = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        d.write_all(text).unwrap();
        assert_eq!(decode_body(&d.finish().unwrap(), "Deflate").unwrap(), text);

        assert!(decode_body(text, "gzip").is_none());
        assert!(decode_body(text, "zstd").is_none());
        assert!(decode_body(text, "identity").is_none());
    }
}
