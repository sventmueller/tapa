//! A minimal HTTP/1.1 client for the OpenCode server on the loopback interface, with basic auth and the event stream.
//!
//! OpenCode's API needs PUT and DELETE besides GET and POST, and its event stream is an answer that never ends. The
//! server only listens on 127.0.0.1, so plain HTTP without TLS is enough, and refusing every other address keeps the
//! per-start password off the network.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

use base64::Engine;
use serde_json::Value;

/// The user name the server expects with its password.
pub(crate) const USER: &str = "opencode";

/// How long connecting to the server may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a request may wait for its answer.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// The largest answer read, such as an exported session.
const BODY_LIMIT: usize = 64 * 1024 * 1024;

/// The longest status or header line accepted.
const LINE_LIMIT: usize = 64 * 1024;

/// Why a request did not get an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// The server's URL is not on 127.0.0.1, or not a URL at all.
    Address(String),
    /// Connecting, writing or reading failed.
    Io {
        /// What sort of failure.
        kind: io::ErrorKind,
        /// The system's words.
        message: String,
    },
    /// The answer is not HTTP as this client reads it.
    Protocol(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Address(url) => write!(f, "{url} is not a server on 127.0.0.1"),
            HttpError::Io { message, .. } => f.write_str(message),
            HttpError::Protocol(why) => write!(f, "unexpected answer: {why}"),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<io::Error> for HttpError {
    fn from(error: io::Error) -> HttpError {
        HttpError::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// An answer with its status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as JSON; an empty one, as a 204 has, is null.
    pub(crate) fn json(&self) -> Result<Value, HttpError> {
        if self.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&self.body).map_err(|e| HttpError::Protocol(e.to_string()))
    }
}

/// The server's address and the credentials every request carries.
#[derive(Clone)]
pub(crate) struct Endpoint {
    addr: SocketAddr,
    authorization: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// `text` with everything but RFC 3986's unreserved characters percent-encoded, for a path segment or query value.
pub(crate) fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The `Authorization` value for `password`.
pub(crate) fn basic(password: &str) -> String {
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{password}"));
    format!("Basic {token}")
}

/// The socket address of `url`, which must be `http://127.0.0.1:<port>`.
pub(crate) fn loopback(url: &str) -> Result<SocketAddr, HttpError> {
    let refused = || HttpError::Address(url.to_string());
    let rest = url.trim().strip_prefix("http://").ok_or_else(refused)?;
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = authority.rsplit_once(':').ok_or_else(refused)?;
    let ip: IpAddr = host.parse().map_err(|_| refused())?;
    if ip != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(refused());
    }
    let port: u16 = port.parse().map_err(|_| refused())?;
    Ok(SocketAddr::new(ip, port))
}

fn read_line(reader: &mut impl BufRead) -> Result<String, HttpError> {
    read_line_within(reader, LINE_LIMIT)
}

/// [`read_line`] for a line of up to `limit` bytes.
fn read_line_within(reader: &mut impl BufRead, limit: usize) -> Result<String, HttpError> {
    let mut line = Vec::new();
    let read = reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(HttpError::Protocol("the connection closed".into()));
    }
    if !line.ends_with(b"\n") {
        return Err(HttpError::Protocol("a line too long".into()));
    }
    let mut text = String::from_utf8(line).map_err(|_| HttpError::Protocol("a line not in UTF-8".into()))?;
    let end = text.trim_end_matches(['\r', '\n']).len();
    text.truncate(end);
    Ok(text)
}

/// The status and the headers, names in lower case.
fn read_head(reader: &mut impl BufRead) -> Result<(u16, Vec<(String, String)>), HttpError> {
    let status_line = read_line(reader)?;
    let mut parts = status_line.splitn(3, ' ');
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return Err(HttpError::Protocol(status_line));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Protocol(status_line));
    }
    let status = code.parse().map_err(|_| HttpError::Protocol(status_line.clone()))?;
    let mut headers = Vec::new();
    loop {
        let line = read_line(reader)?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    Ok((status, headers))
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// The body after `headers` as a reader: chunked, of a known length, or until the server closes.
fn body_reader<R: BufRead + Send + 'static>(reader: R, headers: &[(String, String)]) -> Box<dyn Read + Send> {
    let chunked = header(headers, "transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        return Box::new(Chunked::new(reader));
    }
    match header(headers, "content-length").and_then(|v| v.parse::<u64>().ok()) {
        Some(length) => Box::new(reader.take(length)),
        None => Box::new(reader),
    }
}

/// The body of a `Transfer-Encoding: chunked` answer.
pub(crate) struct Chunked<R> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    pub(crate) fn new(inner: R) -> Chunked<R> {
        Chunked {
            inner,
            left: 0,
            done: false,
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
        let line = read_line(&mut self.inner).map_err(|e| invalid(&e.to_string()))?;
        let size = line.split(';').next().unwrap_or_default().trim();
        self.left = u64::from_str_radix(size, 16).map_err(|_| invalid("a chunk size that is no number"))?;
        if self.left == 0 {
            self.done = true;
            while !read_line(&mut self.inner)
                .map_err(|e| invalid(&e.to_string()))?
                .is_empty()
            {}
        }
        Ok(())
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            self.next_chunk()?;
            if self.done {
                return Ok(0);
            }
        }
        let want = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buf[..want])?;
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the chunk ended early"));
        }
        self.left -= read as u64;
        if self.left == 0 {
            let mut end = [0u8; 2];
            self.inner.read_exact(&mut end)?;
        }
        Ok(read)
    }
}

/// One event of a `text/event-stream`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ServerEvent {
    pub(crate) event: Option<String>,
    pub(crate) data: String,
}

/// The events of a stream as they arrive; `closer` ends it from another thread.
pub(crate) struct Events {
    reader: BufReader<Box<dyn Read + Send>>,
    closer: TcpStream,
}

impl Events {
    /// A handle that ends the stream when it is shut down.
    pub(crate) fn closer(&self) -> io::Result<TcpStream> {
        self.closer.try_clone()
    }

    /// The next event; `None` once the stream ended.
    pub(crate) fn next_event(&mut self) -> Option<ServerEvent> {
        read_event(&mut self.reader)
    }
}

/// The next event of an event stream, per the HTML standard's parsing rules for the fields this API uses; an event
/// whose data outgrows the largest answer ends the stream as a broken one does.
pub(crate) fn read_event(reader: &mut impl BufRead) -> Option<ServerEvent> {
    read_event_within(reader, BODY_LIMIT)
}

/// [`read_event`] with `limit` bytes of data at most.
fn read_event_within(reader: &mut impl BufRead, limit: usize) -> Option<ServerEvent> {
    let mut event = ServerEvent::default();
    let mut has_data = false;
    loop {
        // An event's data comes on one line, so the line may be as long as the data with its field name.
        let line = read_line_within(reader, limit.saturating_add(LINE_LIMIT)).ok()?;
        if line.is_empty() {
            if has_data {
                return Some(event);
            }
            event = ServerEvent::default();
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                if event.data.len() + usize::from(has_data) + value.len() > limit {
                    return None;
                }
                if has_data {
                    event.data.push('\n');
                }
                event.data.push_str(value);
                has_data = true;
            }
            "event" => event.event = Some(value.to_string()),
            _ => {}
        }
    }
}

impl Endpoint {
    /// The server at `url`, which `password` opens.
    pub(crate) fn new(url: &str, password: &str) -> Result<Endpoint, HttpError> {
        Ok(Endpoint {
            addr: loopback(url)?,
            authorization: basic(password),
        })
    }

    fn connect(&self, read_timeout: Option<Duration>) -> Result<TcpStream, HttpError> {
        let stream = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(read_timeout)?;
        stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
        stream.set_nodelay(true)?;
        Ok(stream)
    }

    fn head(&self, method: &str, target: &str, accept: &str, length: Option<usize>) -> String {
        let mut head = format!(
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nAccept: {accept}\r\nConnection: close\r\n",
            self.addr, self.authorization
        );
        if let Some(length) = length {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {length}\r\n"
            ));
        }
        head.push_str("\r\n");
        head
    }

    /// Sends `method` to `target` (path and query) with `body` as JSON and reads the whole answer.
    pub(crate) fn request(&self, method: &str, target: &str, body: Option<&Value>) -> Result<Response, HttpError> {
        let mut stream = self.connect(Some(READ_TIMEOUT))?;
        let payload = body.map(Value::to_string);
        let length = match (&payload, method) {
            (Some(text), _) => Some(text.len()),
            (None, "POST" | "PUT" | "PATCH") => Some(0),
            (None, _) => None,
        };
        let mut message = self.head(method, target, "application/json", length).into_bytes();
        if let Some(text) = &payload {
            message.extend_from_slice(text.as_bytes());
        }
        stream.write_all(&message)?;
        stream.flush()?;
        let mut reader = BufReader::new(stream);
        let (status, headers) = read_head(&mut reader)?;
        let mut body = Vec::new();
        body_reader(reader, &headers)
            .take(BODY_LIMIT as u64 + 1)
            .read_to_end(&mut body)?;
        if body.len() > BODY_LIMIT {
            return Err(HttpError::Protocol("an answer too long".into()));
        }
        Ok(Response { status, body })
    }

    /// Opens the event stream at `target`; an answer other than 200 comes back as the response it is.
    pub(crate) fn events(&self, target: &str) -> Result<Result<Events, Response>, HttpError> {
        let mut stream = self.connect(None)?;
        stream.write_all(self.head("GET", target, "text/event-stream", None).as_bytes())?;
        stream.flush()?;
        let closer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        let (status, headers) = read_head(&mut reader)?;
        let body = body_reader(reader, &headers);
        if status != 200 {
            let mut text = Vec::new();
            body.take(LINE_LIMIT as u64).read_to_end(&mut text)?;
            let _ = closer.shutdown(Shutdown::Both);
            return Ok(Err(Response { status, body: text }));
        }
        Ok(Ok(Events {
            reader: BufReader::new(body),
            closer,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn only_the_loopback_address_is_taken() {
        assert_eq!(
            loopback("http://127.0.0.1:4096").unwrap(),
            "127.0.0.1:4096".parse().unwrap()
        );
        assert_eq!(loopback("http://127.0.0.1:51234/").unwrap().port(), 51234);
        for url in [
            "https://127.0.0.1:4096",
            "http://localhost:4096",
            "http://10.0.0.2:4096",
            "http://example.com:80",
            "http://[::1]:4096",
            "http://127.0.0.1",
        ] {
            assert!(matches!(loopback(url), Err(HttpError::Address(_))), "{url}");
        }
    }

    #[test]
    fn basic_auth_names_the_user_and_values_are_percent_encoded() {
        assert_eq!(basic("pw"), "Basic b3BlbmNvZGU6cHc=");
        assert_eq!(encode("/Users/a b/ä"), "%2FUsers%2Fa%20b%2F%C3%A4");
        assert_eq!(encode("ses_1-a.b~"), "ses_1-a.b~");
    }

    #[test]
    fn a_chunked_body_reads_whole_and_ends_at_the_last_chunk() {
        let wire = b"5\r\nhello\r\n7;ext=1\r\n, world\r\n0\r\nX-Trailer: 1\r\n\r\nafter";
        let mut text = String::new();
        Chunked::new(Cursor::new(&wire[..])).read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello, world");
        let broken = b"zz\r\nhello\r\n";
        assert!(
            Chunked::new(Cursor::new(&broken[..]))
                .read_to_string(&mut String::new())
                .is_err()
        );
    }

    #[test]
    fn events_join_their_data_lines_and_skip_comments() {
        let wire = ": ping\n\nid: evt_1\nevent: session.idle\ndata: {\"a\":\ndata: 1}\n\nevent: empty\n\ndata:x\n\n";
        let mut reader = Cursor::new(wire.as_bytes());
        let first = read_event(&mut reader).unwrap();
        assert_eq!(first.event.as_deref(), Some("session.idle"));
        assert_eq!(first.data, "{\"a\":\n1}");
        let second = read_event(&mut reader).unwrap();
        assert_eq!((second.event, second.data.as_str()), (None, "x"));
        assert_eq!(read_event(&mut reader), None);
    }

    #[test]
    fn an_event_is_held_to_the_limit_across_its_data_lines() {
        let wire = "data: 1234\ndata: 5\n\ndata: 1234\ndata: 56\n\ndata: next\n\n";
        let mut reader = Cursor::new(wire.as_bytes());
        assert_eq!(read_event_within(&mut reader, 6).unwrap().data, "1234\n5");
        assert_eq!(read_event_within(&mut reader, 6), None);
    }

    #[test]
    fn an_event_longer_than_a_header_line_is_read_whole() {
        let data = "x".repeat(LINE_LIMIT * 2);
        let wire = format!("data: {data}\n\ndata: next\n\n");
        let mut reader = Cursor::new(wire.as_bytes());
        assert_eq!(read_event(&mut reader).unwrap().data, data);
        assert_eq!(read_event(&mut reader).unwrap().data, "next");
    }

    #[test]
    fn a_line_loses_only_its_line_ending() {
        let mut reader = Cursor::new(&b"data: a \r\n\n"[..]);
        assert_eq!(read_line(&mut reader).unwrap(), "data: a ");
        assert_eq!(read_line(&mut reader).unwrap(), "");
    }

    #[test]
    fn the_head_is_read_with_lower_case_names() {
        let mut reader = Cursor::new(&b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..]);
        let (status, headers) = read_head(&mut reader).unwrap();
        assert_eq!(status, 204);
        assert_eq!(header(&headers, "content-length"), Some("0"));
        let mut reader = Cursor::new(&b"SSH-2.0\r\n\r\n"[..]);
        assert!(matches!(read_head(&mut reader), Err(HttpError::Protocol(_))));
        let empty = Response {
            status: 204,
            body: Vec::new(),
        };
        assert_eq!(empty.json().unwrap(), Value::Null);
    }
}
