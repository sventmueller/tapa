//! WebSocket framing (RFC 6455) for the shared server (E148): its control socket, which `codex app-server proxy`
//! hands through byte for byte, upgrades each connection to a WebSocket and carries one JSON-RPC message per text
//! frame. Only what that connection needs: the upgrade, text, ping, pong, close and continuation frames.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, Read};

use base64::Engine;

/// A text frame.
pub(crate) const TEXT: u8 = 0x1;
/// A pong, the answer to a ping.
pub(crate) const PONG: u8 = 0xA;
/// A ping, which wants a pong with its payload.
pub(crate) const PING: u8 = 0x9;
/// The next part of a message split into frames.
pub(crate) const CONTINUATION: u8 = 0x0;
const BINARY: u8 = 0x2;
const CLOSE: u8 = 0x8;

/// The largest message taken in; a longer one ends the connection.
pub(crate) const MAX_MESSAGE: usize = 256 * 1024 * 1024;

/// The longest answer to the upgrade request taken in.
const MAX_HEAD: u64 = 16 * 1024;

/// Four bytes nobody can predict, for a mask or a key.
fn random() -> [u8; 4] {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    let bits = hasher.finish().to_le_bytes();
    [bits[0], bits[1], bits[2], bits[3]]
}

/// The request that turns the connection into a WebSocket.
pub(crate) fn upgrade_request() -> Vec<u8> {
    let key: Vec<u8> = (0..4).flat_map(|_| random()).collect();
    let key = base64::engine::general_purpose::STANDARD.encode(key);
    format!(
        "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .into_bytes()
}

/// Reads the answer to the upgrade request up to its blank line; an error unless it switches protocols.
pub(crate) fn read_upgrade(reader: &mut impl BufRead) -> io::Result<()> {
    let mut head = reader.by_ref().take(MAX_HEAD);
    let mut status = String::new();
    head.read_line(&mut status)?;
    loop {
        let mut line = String::new();
        if head.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the upgrade's answer ended early",
            ));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    if status.split_whitespace().nth(1) == Some("101") {
        Ok(())
    } else {
        Err(io::Error::other(format!("the upgrade was refused: {}", status.trim())))
    }
}

/// Reads a request to upgrade, up to its blank line, as the server's side does.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn read_request(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the upgrade request ended early",
            ));
        }
        if line == "\r\n" || line == "\n" {
            return Ok(());
        }
    }
}

/// The server's answer that accepts an upgrade; the key's check is left out, as the client does not read it.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn upgrade_answer() -> &'static [u8] {
    b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"
}

/// A whole frame of `opcode` with `payload`; a client's is masked, as RFC 6455 requires of it.
pub(crate) fn frame(opcode: u8, payload: &[u8], masked: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    let mask_bit = if masked { 0x80 } else { 0 };
    match payload.len() {
        len @ 0..=125 => out.push(mask_bit | u8::try_from(len).unwrap_or(125)),
        len @ 126..=0xFFFF => {
            out.push(mask_bit | 126);
            out.extend_from_slice(&u16::try_from(len).unwrap_or(u16::MAX).to_be_bytes());
        }
        len => {
            out.push(mask_bit | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    if masked {
        let mask = random();
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    } else {
        out.extend_from_slice(payload);
    }
    out
}

/// One frame as it came: whether it ends its message, its opcode and its unmasked payload, and whether it was masked.
struct Frame {
    fin: bool,
    opcode: u8,
    masked: bool,
    payload: Vec<u8>,
}

fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0u8; 2];
    reader.read_exact(&mut head)?;
    let masked = head[1] & 0x80 != 0;
    let len = match head[1] & 0x7F {
        126 => {
            let mut len = [0u8; 2];
            reader.read_exact(&mut len)?;
            u64::from(u16::from_be_bytes(len))
        }
        127 => {
            let mut len = [0u8; 8];
            reader.read_exact(&mut len)?;
            u64::from_be_bytes(len)
        }
        len => u64::from(len),
    };
    let len = usize::try_from(len)
        .ok()
        .filter(|len| *len <= MAX_MESSAGE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a frame is too long"))?;
    let mut mask = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    Ok(Frame {
        fin: head[0] & 0x80 != 0,
        opcode: head[0] & 0x0F,
        masked,
        payload,
    })
}

/// What a peer must do with its frames: a client's come masked, a server's not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Peer {
    /// The frames come from the server.
    Server,
    /// The frames come from a client.
    Client,
}

/// Reads the next whole text message, joining its continuation frames; `control` hears each ping and pong with its
/// payload, for the caller to answer a ping with a pong. `None` once the other side closed the connection.
pub(crate) fn read_message(
    reader: &mut impl Read,
    from: Peer,
    control: &mut dyn FnMut(u8, Vec<u8>),
) -> io::Result<Option<String>> {
    let mut message: Vec<u8> = Vec::new();
    loop {
        let frame = match read_frame(reader) {
            Ok(frame) => frame,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        };
        if frame.masked != (from == Peer::Client) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a frame is masked the wrong way",
            ));
        }
        match frame.opcode {
            CLOSE => return Ok(None),
            PING | PONG => control(frame.opcode, frame.payload),
            TEXT | BINARY | CONTINUATION => {
                if message.len() + frame.payload.len() > MAX_MESSAGE {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "a message is too long"));
                }
                message.extend_from_slice(&frame.payload);
                if frame.fin {
                    return String::from_utf8(message)
                        .map(Some)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_as_rfc_6455_writes_them() {
        // The examples of RFC 6455, section 5.7: "Hello" unmasked, masked, and in two fragments.
        let unmasked = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        let masked = [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        let fragments = [0x01, 0x03, 0x48, 0x65, 0x6c, 0x80, 0x02, 0x6c, 0x6f];
        let mut none = |_: u8, _: Vec<u8>| panic!("no ping");
        let read = |bytes: &[u8], from: Peer, none: &mut dyn FnMut(u8, Vec<u8>)| {
            read_message(&mut &bytes[..], from, none).unwrap()
        };
        assert_eq!(read(&unmasked, Peer::Server, &mut none).as_deref(), Some("Hello"));
        assert_eq!(read(&masked, Peer::Client, &mut none).as_deref(), Some("Hello"));
        assert_eq!(read(&fragments, Peer::Server, &mut none).as_deref(), Some("Hello"));
        assert!(read_message(&mut &unmasked[..], Peer::Client, &mut none).is_err());
        assert_eq!(frame(TEXT, b"Hello", false), unmasked);
    }

    #[test]
    fn a_ping_between_fragments_is_heard_and_long_frames_keep_their_length() {
        let mut bytes = vec![
            0x01, 0x03, 0x48, 0x65, 0x6c, 0x89, 0x02, 0x68, 0x69, 0x80, 0x02, 0x6c, 0x6f,
        ];
        let long = "x".repeat(70_000);
        bytes.extend(frame(TEXT, long.as_bytes(), true));
        bytes.extend(frame(TEXT, &[b'y'; 300], false));
        bytes.extend(frame(CLOSE, &[], false));
        let mut reader = &bytes[..];
        let mut pings = Vec::new();
        let mut heard = |opcode: u8, payload: Vec<u8>| pings.push((opcode, payload));
        assert_eq!(
            read_message(&mut reader, Peer::Server, &mut heard).unwrap().as_deref(),
            Some("Hello")
        );
        assert!(
            read_message(&mut reader, Peer::Server, &mut heard).is_err(),
            "a masked frame from the server"
        );
        let mut reader = &frame(TEXT, long.as_bytes(), true)[..];
        assert_eq!(read_message(&mut reader, Peer::Client, &mut heard).unwrap(), Some(long));
        let mut reader = &bytes[bytes.len() - 306..];
        assert_eq!(
            read_message(&mut reader, Peer::Server, &mut heard).unwrap(),
            Some("y".repeat(300))
        );
        assert_eq!(read_message(&mut reader, Peer::Server, &mut heard).unwrap(), None);
        assert_eq!(pings, [(PING, b"hi".to_vec())]);
    }

    #[test]
    fn the_upgrade_is_asked_for_and_its_answer_read() {
        let request = String::from_utf8(upgrade_request()).unwrap();
        assert!(request.starts_with("GET / HTTP/1.1\r\n") && request.ends_with("\r\n\r\n"));
        assert!(request.contains("Upgrade: websocket\r\n") && request.contains("Sec-WebSocket-Version: 13\r\n"));
        read_request(&mut request.as_bytes()).unwrap();
        let mut answer = upgrade_answer();
        read_upgrade(&mut answer).unwrap();
        assert!(read_upgrade(&mut &b"HTTP/1.1 403 Forbidden\r\n\r\n"[..]).is_err());
        assert!(read_upgrade(&mut &b"HTTP/1.1 101 Switching"[..]).is_err());
    }

    #[test]
    fn the_upgrade_answer_is_read_within_its_limit_and_leaves_the_frames_after_it() {
        let endless = &mut io::BufReader::new(io::repeat(b'x'));
        assert!(read_upgrade(endless).is_err(), "a status line without end");
        let mut bytes = upgrade_answer().to_vec();
        bytes.extend(frame(TEXT, b"Hello", false));
        let mut reader = &bytes[..];
        read_upgrade(&mut reader).unwrap();
        let mut none = |_: u8, _: Vec<u8>| panic!("no ping");
        assert_eq!(
            read_message(&mut reader, Peer::Server, &mut none).unwrap().as_deref(),
            Some("Hello")
        );
    }
}
