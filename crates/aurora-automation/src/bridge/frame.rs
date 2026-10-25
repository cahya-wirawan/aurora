//! Bounded line reading and writing.
//!
//! A peer chooses how long its line is; the reader must not. Reading stops
//! the moment a line would pass the limit (nothing past the limit is
//! buffered), a deadline bounds the whole line (not each read, so a client
//! that drips one byte every few seconds cannot hold a connection open),
//! and a line that is not terminated is never returned.

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// What reading a line gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A complete line, without its terminator (`\n` or `\r\n`).
    Data(Vec<u8>),
    /// The peer closed (an unterminated partial line is discarded).
    Eof,
    /// The line passed the limit. The stream is no longer in step: the
    /// caller must close it and must not act on anything read.
    TooLong,
}

/// Reads one line of at most `max` bytes from `reader`.
///
/// # Errors
/// Any read error, including the timeout kinds when the reader is a socket
/// with a read timeout.
pub fn read_line(reader: &mut impl BufRead, max: usize) -> io::Result<Line> {
    read_line_until(reader, max, &mut |_| Ok(()))
}

/// [`read_line`] that calls `before_read` with the bytes read so far ahead
/// of every blocking read; it may return an error (a deadline) to stop.
fn read_line_until(
    reader: &mut impl BufRead,
    max: usize,
    before_read: &mut dyn FnMut(usize) -> io::Result<()>,
) -> io::Result<Line> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        before_read(line.len())?;
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if buffer.is_empty() {
            return Ok(Line::Eof);
        }
        if let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            if line.len().saturating_add(end) > max {
                return Ok(Line::TooLong);
            }
            if let Some(part) = buffer.get(..end) {
                line.extend_from_slice(part);
            }
            reader.consume(end + 1);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Line::Data(line));
        }
        let taken = buffer.len();
        if line.len().saturating_add(taken) > max {
            return Ok(Line::TooLong);
        }
        line.extend_from_slice(buffer);
        reader.consume(taken);
    }
}

/// Reads one line from a socket, giving up when `deadline` passes (the
/// whole line, however slowly it arrives).
///
/// # Errors
/// `TimedOut` at the deadline, or any read error.
pub fn read_line_by(
    reader: &mut BufReader<TcpStream>,
    max: usize,
    deadline: Instant,
) -> io::Result<Line> {
    // The deadline is checked before every blocking read and bounds that
    // read through the socket's own timeout.
    let socket = reader.get_ref().try_clone()?;
    read_line_until(reader, max, &mut |_| {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "line deadline"));
        }
        let remaining = deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(1));
        socket.set_read_timeout(Some(remaining))
    })
}

/// Writes `text` and a newline, as one write, and flushes.
///
/// # Errors
/// A write error, or `text` containing a newline (it would split the line).
pub fn write_line(writer: &mut impl Write, text: &str) -> io::Result<()> {
    if text.contains('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a line may not contain a newline",
        ));
    }
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_lines_and_strips_the_terminator() {
        let mut r = Cursor::new(b"abc\r\ndef\n".to_vec());
        assert_eq!(read_line(&mut r, 16).unwrap(), Line::Data(b"abc".to_vec()));
        assert_eq!(read_line(&mut r, 16).unwrap(), Line::Data(b"def".to_vec()));
        assert_eq!(read_line(&mut r, 16).unwrap(), Line::Eof);
    }

    #[test]
    fn a_line_at_the_limit_passes_and_one_over_does_not() {
        let mut ok = Cursor::new(b"12345\n".to_vec());
        assert_eq!(
            read_line(&mut ok, 5).unwrap(),
            Line::Data(b"12345".to_vec())
        );
        let mut over = Cursor::new(b"123456\n".to_vec());
        assert_eq!(read_line(&mut over, 5).unwrap(), Line::TooLong);
    }

    #[test]
    fn an_endless_line_stops_at_the_limit_without_buffering_it_all() {
        struct Endless(u64);
        impl io::Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0 += buf.len() as u64;
                buf.fill(b'a');
                Ok(buf.len())
            }
        }
        let mut r = BufReader::with_capacity(8192, Endless(0));
        assert_eq!(read_line(&mut r, 1000).unwrap(), Line::TooLong);
        // It read a handful of buffers, not the stream.
        assert!(r.get_ref().0 < 100_000);
    }

    #[test]
    fn an_unterminated_line_is_discarded() {
        let mut r = Cursor::new(b"partial".to_vec());
        assert_eq!(read_line(&mut r, 100).unwrap(), Line::Eof);
    }

    #[test]
    fn a_line_cannot_carry_a_newline_out() {
        let mut out = Vec::new();
        assert!(write_line(&mut out, "a\nb").is_err());
        write_line(&mut out, "ab").unwrap();
        assert_eq!(out, b"ab\n");
    }
}
