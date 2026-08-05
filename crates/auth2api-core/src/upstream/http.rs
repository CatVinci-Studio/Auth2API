//! Shared HTTP client construction for every upstream request.

use std::sync::RwLock;
use std::time::Duration;

static PROXY_URL: RwLock<Option<String>> = RwLock::new(None);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Max silence between two chunks of a streamed body before giving up.
/// Generous enough for a reasoning model thinking between tokens, short
/// enough that a dead proxy doesn't pin a connection open forever.
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(180);

/// Sets (or clears, with None/empty) the proxy every upstream request routes
/// through. Rejects unparseable URLs so a typo in `config.toml` fails at
/// startup, visibly, instead of silently at request time.
pub fn set_proxy(url: Option<String>) -> Result<(), String> {
    let url = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    if let Some(u) = &url {
        reqwest::Proxy::all(u.clone()).map_err(|e| format!("invalid proxy URL {u:?}: {e}"))?;
    }
    *PROXY_URL.write().unwrap() = url;
    Ok(())
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
}

fn streaming_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STREAM_READ_TIMEOUT)
}

fn with_proxy(build: fn() -> reqwest::ClientBuilder) -> reqwest::Client {
    PROXY_URL
        .read()
        .unwrap()
        .clone()
        .and_then(|u| reqwest::Proxy::all(u).ok())
        .and_then(|p| build().proxy(p).build().ok())
        .or_else(|| build().build().ok())
        .unwrap_or_default()
}

/// For whole-body requests (the OAuth token endpoints).
pub fn client() -> reqwest::Client {
    with_proxy(builder)
}

/// For SSE-streamed model requests: no total timeout (a long healthy stream
/// must not be cut off mid-answer), only a per-read idle timeout.
pub fn streaming_client() -> reqwest::Client {
    with_proxy(streaming_builder)
}

/// Reassembles `data:` payload lines from raw SSE body chunks, whose
/// boundaries can land anywhere - mid-line, even mid-way through a multi-byte
/// UTF-8 character - so bytes are buffered and only complete lines (which SSE
/// guarantees are whole UTF-8 units) get decoded.
#[derive(Default)]
pub struct SseLineBuffer {
    buf: Vec<u8>,
}

impl SseLineBuffer {
    pub fn push(&mut self, chunk: &[u8], mut on_data: impl FnMut(&str)) {
        self.buf.extend_from_slice(chunk);
        while let Some(newline) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(data) = line.strip_prefix("data:") {
                on_data(data.strip_prefix(' ').unwrap_or(data));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(buffer: &mut SseLineBuffer, chunk: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        buffer.push(chunk, |data| out.push(data.to_string()));
        out
    }

    #[test]
    fn emits_each_data_line_and_ignores_other_fields() {
        let mut buffer = SseLineBuffer::default();
        let got = collect(
            &mut buffer,
            b"event: message\ndata: {\"a\":1}\n\ndata:{\"b\":2}\n",
        );
        assert_eq!(got, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn reassembles_lines_split_across_chunks() {
        let mut buffer = SseLineBuffer::default();
        assert!(collect(&mut buffer, b"data: {\"te").is_empty());
        assert_eq!(
            collect(&mut buffer, b"xt\":\"hi\"}\n"),
            vec!["{\"text\":\"hi\"}"]
        );
    }

    #[test]
    fn handles_chunk_boundary_inside_multibyte_char() {
        let payload = "data: {\"text\":\"中文\"}\n".as_bytes();
        let split = payload.iter().position(|&b| b > 0x7f).unwrap() + 1;
        let mut buffer = SseLineBuffer::default();
        assert!(collect(&mut buffer, &payload[..split]).is_empty());
        assert_eq!(
            collect(&mut buffer, &payload[split..]),
            vec!["{\"text\":\"中文\"}"]
        );
    }

    #[test]
    fn strips_crlf_line_endings() {
        let mut buffer = SseLineBuffer::default();
        assert_eq!(collect(&mut buffer, b"data: [DONE]\r\n"), vec!["[DONE]"]);
    }
}
