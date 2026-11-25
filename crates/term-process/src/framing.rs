//! The framing the Language Server Protocol and the Debug Adapter Protocol share on a process's standard streams: each
//! message is a header block with its byte length, a blank line, then the JSON. Also the capped line reading that
//! line-based protocols use.

use std::io::{self, BufRead, Read};

/// The largest message accepted; a header claiming more means the stream is broken.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;

/// The space set aside for a message body before its bytes arrive.
const FIRST_SPACE: usize = 1024 * 1024;

/// The longest header line kept; a longer line is no header but output the server printed by mistake.
const MAX_HEADER_LINE: usize = 64 * 1024;

/// What [`read_line`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// A line, in the buffer with its newline; the last one of a stream may lack it.
    Read,
    /// A line longer than the limit, read up to its newline and dropped; the buffer is empty.
    TooLong,
    /// The stream ended before another line.
    End,
}

/// Reads one line into `line`, which it empties first; a line of more than `limit` bytes before its newline is consumed
/// to its end and dropped, so that one runaway line costs neither the memory nor the lines after it.
pub fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>, limit: usize) -> io::Result<Line> {
    line.clear();
    let mut too_long = false;
    let mut any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(match (any, too_long) {
                (false, _) => Line::End,
                (true, true) => Line::TooLong,
                (true, false) => Line::Read,
            });
        }
        any = true;
        let newline = available.iter().position(|&b| b == b'\n');
        let take = newline.map_or(available.len(), |at| at + 1);
        if !too_long && line.len() + take - usize::from(newline.is_some()) > limit {
            too_long = true;
            line.clear();
        }
        if !too_long {
            line.extend_from_slice(&available[..take]);
        }
        reader.consume(take);
        if newline.is_some() {
            return Ok(if too_long { Line::TooLong } else { Line::Read });
        }
    }
}

/// The next message body, or `None` when the stream ends between messages.
///
/// Output before a header that is not one is skipped, as some servers print stray lines to standard output: lines
/// without a colon, overlong lines, and a block of lines with colons that ends without `Content-Length`.
pub fn read(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut line = Vec::new();
    let length = loop {
        let text = match read_line(reader, &mut line, MAX_HEADER_LINE)? {
            Line::End if length.is_some() => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended inside a header",
                ));
            }
            Line::End => return Ok(None),
            Line::TooLong if length.is_some() => return Err(invalid("a header line that is too long")),
            Line::TooLong => continue,
            Line::Read => String::from_utf8_lossy(&line),
        };
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            match length {
                Some(length) => break length,
                None => continue,
            }
        }
        let Some((name, value)) = text.split_once(':') else {
            if length.is_some() {
                return Err(invalid("a header line without a colon"));
            }
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid("Content-Length is not a number"))?;
            if n > MAX_MESSAGE {
                return Err(invalid("the message is too large"));
            }
            length = Some(n);
        }
    };
    // The header alone is no reason to set aside up to MAX_MESSAGE; the body grows with what really arrives.
    let mut body = Vec::with_capacity(length.min(FIRST_SPACE));
    Read::take(&mut *reader, u64::try_from(length).unwrap_or(u64::MAX)).read_to_end(&mut body)?;
    if body.len() < length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the stream ended inside a message",
        ));
    }
    Ok(Some(body))
}

/// `body` with its header, ready to write in one piece.
pub fn frame(body: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut bytes = Vec::with_capacity(header.len() + body.len());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(body);
    bytes
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read};

    use super::*;

    /// Hands out its bytes a few at a time, as a pipe does under load.
    struct Trickle {
        bytes: Vec<u8>,
        at: usize,
        step: usize,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.bytes.len() - self.at);
            buf[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    fn trickle(bytes: Vec<u8>, step: usize) -> BufReader<Trickle> {
        BufReader::with_capacity(4, Trickle { bytes, at: 0, step })
    }

    #[test]
    fn messages_arriving_in_pieces_are_put_back_together() {
        let first = r#"{"jsonrpc":"2.0","method":"a"}"#;
        let second = r#"{"jsonrpc":"2.0","method":"b"}"#;
        let mut bytes = frame(first.as_bytes());
        bytes.extend(frame(second.as_bytes()));
        for step in [1, 2, 3, 7, 64] {
            let mut reader = trickle(bytes.clone(), step);
            assert_eq!(read(&mut reader).unwrap().unwrap(), first.as_bytes());
            assert_eq!(read(&mut reader).unwrap().unwrap(), second.as_bytes());
            assert_eq!(read(&mut reader).unwrap(), None);
        }
    }

    #[test]
    fn the_length_counts_bytes_so_multibyte_text_survives() {
        let body = r#"{"text":"Grüße 😀 ∑"}"#;
        assert_ne!(body.len(), body.chars().count());
        let mut reader = trickle(frame(body.as_bytes()), 1);
        assert_eq!(read(&mut reader).unwrap().unwrap(), body.as_bytes());
    }

    #[test]
    fn other_headers_and_their_case_do_not_matter() {
        let bytes = b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}".to_vec();
        assert_eq!(read(&mut trickle(bytes, 5)).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn stray_lines_before_a_message_are_skipped() {
        let mut bytes = b"\r\nnode warning printed to stdout\n".to_vec();
        bytes.extend(frame(b"{}"));
        assert_eq!(read(&mut trickle(bytes, 3)).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn stray_lines_with_a_colon_and_a_blank_line_are_skipped_up_to_the_next_header() {
        let mut bytes = b"warning: something odd\n\nnote: twice\r\n\r\n".to_vec();
        bytes.extend(frame(b"{}"));
        bytes.extend(b"Content-Type: x\r\n\r\n{}");
        let mut reader = trickle(bytes, 3);
        assert_eq!(read(&mut reader).unwrap().unwrap(), b"{}");
        assert_eq!(read(&mut reader).unwrap(), None);
    }

    #[test]
    fn an_overlong_stray_line_is_skipped_without_keeping_it() {
        let mut bytes = vec![b'x'; MAX_HEADER_LINE + 10];
        bytes.extend(b": not a header\n");
        bytes.extend(frame(b"[1]"));
        assert_eq!(read(&mut trickle(bytes, 4096)).unwrap().unwrap(), b"[1]");
        let mut long_header = b"Content-Length: 2\r\n".to_vec();
        long_header.extend(vec![b'y'; MAX_HEADER_LINE + 1]);
        long_header.extend(b"\r\n\r\n{}");
        assert!(read(&mut trickle(long_header, 4096)).is_err());
    }

    #[test]
    fn lines_are_capped_and_the_last_may_lack_its_newline() {
        let mut reader = trickle(b"short\nwaytoolong\ncut off".to_vec(), 3);
        let mut line = Vec::new();
        assert_eq!(read_line(&mut reader, &mut line, 5).unwrap(), Line::Read);
        assert_eq!(line, b"short\n");
        assert_eq!(read_line(&mut reader, &mut line, 5).unwrap(), Line::TooLong);
        assert!(line.is_empty());
        assert_eq!(read_line(&mut reader, &mut line, 8).unwrap(), Line::Read);
        assert_eq!(line, b"cut off");
        assert_eq!(read_line(&mut reader, &mut line, 8).unwrap(), Line::End);
        let mut overlong_at_the_end = trickle(b"0123456789".to_vec(), 4);
        assert_eq!(
            read_line(&mut overlong_at_the_end, &mut line, 5).unwrap(),
            Line::TooLong
        );
        assert_eq!(read_line(&mut overlong_at_the_end, &mut line, 5).unwrap(), Line::End);
    }

    #[test]
    fn a_broken_header_is_an_error_not_a_hang() {
        let huge = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1).into_bytes();
        assert!(read(&mut trickle(huge, 4)).is_err());
        let cut = b"Content-Length: 10\r\n\r\n{}".to_vec();
        assert!(read(&mut trickle(cut, 4)).is_err());
        let cut_header = b"Content-Length: 10\r\n".to_vec();
        assert!(read(&mut trickle(cut_header, 4)).is_err());
    }
}
