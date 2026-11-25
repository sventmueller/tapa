//! The server's side of HTTP/1.1 for the stand-in: one request per connection, answers sized or chunked.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use serde_json::Value;

/// The longest body the stand-in reads.
const BODY_LIMIT: u64 = 16 * 1024 * 1024;

/// A request as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Request {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) query: Vec<(String, String)>,
    pub(super) authorization: Option<String>,
    pub(super) body: Value,
}

impl Request {
    /// The path's segments after `/api`.
    pub(super) fn segments(&self) -> Vec<String> {
        self.path
            .trim_start_matches("/api")
            .split('/')
            .filter(|s| !s.is_empty())
            .map(decode)
            .collect()
    }

    pub(super) fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// `text` with its percent escapes resolved.
pub(super) fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut text = String::new();
    reader.take(64 * 1024).read_line(&mut text)?;
    Ok(text.trim_end_matches(['\r', '\n']).to_string())
}

/// Reads one request from `stream`.
pub(super) fn read(stream: &TcpStream) -> io::Result<Request> {
    let mut reader = BufReader::new(stream);
    let request_line = line(&mut reader)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, request_line));
    };
    let mut length = 0u64;
    let mut authorization = None;
    loop {
        let header = line(&mut reader)?;
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or_default(),
            "authorization" => authorization = Some(value.trim().to_string()),
            _ => {}
        }
    }
    let mut body = Vec::new();
    reader.take(length.min(BODY_LIMIT)).read_to_end(&mut body)?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok(Request {
        method: method.to_string(),
        path: path.to_string(),
        query: query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (name, value) = p.split_once('=').unwrap_or((p, ""));
                (decode(name), decode(value))
            })
            .collect(),
        authorization,
        body: if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        },
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Bad Request",
    }
}

/// Answers with `status` and `body`, chunked when `chunked`; a 204 has no body.
pub(super) fn respond(mut stream: &TcpStream, status: u16, body: Option<&Value>, chunked: bool) -> io::Result<()> {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut head = format!("HTTP/1.1 {status} {}\r\nConnection: close\r\n", reason(status));
    if status == 204 {
        head.push_str("\r\n");
        return stream.write_all(head.as_bytes());
    }
    head.push_str("Content-Type: application/json\r\n");
    if chunked {
        head.push_str("Transfer-Encoding: chunked\r\n\r\n");
        let mut message = head.into_bytes();
        for piece in text.as_bytes().chunks(97) {
            message.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            message.extend_from_slice(piece);
            message.extend_from_slice(b"\r\n");
        }
        message.extend_from_slice(b"0\r\n\r\n");
        return stream.write_all(&message);
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", text.len()));
    stream.write_all(head.as_bytes())?;
    stream.write_all(text.as_bytes())
}

/// Starts an event stream on `stream`.
pub(super) fn begin_events(mut stream: &TcpStream) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n",
    )
}

/// Writes `frame` as one chunk of the event stream.
pub(super) fn chunk(mut stream: &TcpStream, frame: &str) -> io::Result<()> {
    stream.write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_resolve() {
        assert_eq!(decode("location%5Bdirectory%5D"), "location[directory]");
        assert_eq!(decode("%2Fa%20b%C3%A4"), "/a bä");
        assert_eq!(decode("100%"), "100%");
    }
}
