//! The iiod text protocol over one TCP connection.
//!
//! Every command is one line ending in `\r\n`; the server answers with a decimal status line (a length, or a
//! negative errno) followed by the payload:
//!
//! | command | answer |
//! | --- | --- |
//! | `VERSION` | `0.25.gabcdef0\n` |
//! | `PRINT` | `LEN\n` + XML + `\n` |
//! | `TIMEOUT MS` | `0\n` |
//! | `READ DEV [INPUT\|OUTPUT CHN \| DEBUG \| BUFFER] ATTR` | `LEN\n` + value + `\n` |
//! | `WRITE DEV [...] ATTR LEN` + value | `LEN\n` |
//! | `SET DEV BUFFERS_COUNT N` | `0\n` |
//! | `OPEN DEV SAMPLES MASK [CYCLIC]` | `0\n` |
//! | `READBUF DEV BYTES` | (`LEN\n` [+ `MASK\n` the first time] + data) until BYTES are sent |
//! | `WRITEBUF DEV BYTES` | `BYTES\n`, then the client sends the data, then `BYTES\n` |
//! | `CLOSE DEV` | `0\n` |

use crate::{Context, Error};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The port iiod listens on.
pub const DEFAULT_PORT: u16 = 30431;

/// Which attribute of a device a `READ` or `WRITE` addresses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Attr<'a> {
    Device(&'a str),
    Debug(&'a str),
    Buffer(&'a str),
    Channel {
        channel: &'a str,
        output: bool,
        attr: &'a str,
    },
}

impl Attr<'_> {
    /// The words between the device and the end of a `READ`/`WRITE` command.
    fn words(&self) -> String {
        match self {
            Attr::Device(attr) => attr.to_string(),
            Attr::Debug(attr) => format!("DEBUG {attr}"),
            Attr::Buffer(attr) => format!("BUFFER {attr}"),
            Attr::Channel {
                channel,
                output,
                attr,
            } => format!(
                "{} {channel} {attr}",
                if *output { "OUTPUT" } else { "INPUT" }
            ),
        }
    }
}

impl std::fmt::Display for Attr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Attr::Device(attr) => write!(f, "{attr}"),
            Attr::Debug(attr) => write!(f, "debug:{attr}"),
            Attr::Buffer(attr) => write!(f, "buffer:{attr}"),
            Attr::Channel {
                channel,
                output,
                attr,
            } => write!(f, "{}:{channel} {attr}", if *output { "out" } else { "in" }),
        }
    }
}

/// One connection to an iiod server.
pub struct Client {
    stream: BufReader<TcpStream>,
}

impl Client {
    /// Connects to `host:port` (the first address that answers within `timeout`), with `timeout` also as the
    /// I/O timeout until [`set_timeout`](Self::set_timeout) changes it.
    pub fn connect(addr: impl ToSocketAddrs, timeout: Duration) -> Result<Client, Error> {
        let mut last = None;
        for addr in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(stream) => {
                    stream.set_nodelay(true)?;
                    stream.set_read_timeout(Some(timeout))?;
                    stream.set_write_timeout(Some(timeout))?;
                    return Ok(Client {
                        stream: BufReader::new(stream),
                    });
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last
            .unwrap_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no address to connect to")
            })
            .into())
    }

    /// Sets how long one read or write may take, here and (slightly shorter) in the server, so that a capture
    /// waiting for a trigger ends with the server's `ETIMEDOUT` rather than a dead socket.
    pub fn set_timeout(&mut self, timeout: Duration) -> Result<(), Error> {
        let ms = timeout.as_millis().clamp(1, u128::from(u32::MAX));
        self.command(&format!("TIMEOUT {ms}"))?;
        self.status("TIMEOUT")?;
        let local = Some(timeout + Duration::from_secs(1));
        self.stream.get_ref().set_read_timeout(local)?;
        self.stream.get_ref().set_write_timeout(local)?;
        Ok(())
    }

    /// The server's version line, e.g. `0.25.g1234abc`.
    pub fn version(&mut self) -> Result<String, Error> {
        self.command("VERSION")?;
        self.line()
    }

    pub fn context_xml(&mut self) -> Result<String, Error> {
        self.command("PRINT")?;
        let len = self.status("PRINT")?;
        let xml = self.payload(len)?;
        String::from_utf8(xml).map_err(|_| Error::Protocol("context XML is not UTF-8".into()))
    }

    pub fn context(&mut self) -> Result<Context, Error> {
        Context::parse(&self.context_xml()?)
    }

    /// Reads an attribute of device `dev` (its id, e.g. `iio:device0`; not every server takes names).
    pub fn read_attr(&mut self, dev: &str, attr: Attr) -> Result<String, Error> {
        let what = format!("read {dev} {attr}");
        self.command(&format!("READ {dev} {}", attr.words()))?;
        let len = self.status(&what)?;
        let value = self.payload(len)?;
        let text = String::from_utf8_lossy(&value);
        Ok(text
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string())
    }

    pub fn write_attr(&mut self, dev: &str, attr: Attr, value: &str) -> Result<(), Error> {
        let what = format!("write {dev} {attr} = {value}");
        // like libiio, the value goes with its terminating NUL
        let mut data = format!(
            "WRITE {dev} {} {}\r\n{value}",
            attr.words(),
            value.len() + 1
        )
        .into_bytes();
        data.push(0);
        self.send(&data)?;
        self.status(&what)?;
        Ok(())
    }

    /// Sets the number of kernel buffers of `dev`; before [`open`](Self::open).
    pub fn set_buffers_count(&mut self, dev: &str, count: u32) -> Result<(), Error> {
        self.command(&format!("SET {dev} BUFFERS_COUNT {count}"))?;
        self.status(&format!("set {dev} buffers count"))?;
        Ok(())
    }

    /// Opens the buffer of `dev` for `samples` samples of the channels in `mask` (least significant word first,
    /// see [`Device::mask_of`](crate::Device::mask_of)). The buffer lives until [`close`](Self::close) or the
    /// end of the connection; a cyclic output buffer repeats its data that long.
    pub fn open(
        &mut self,
        dev: &str,
        samples: usize,
        mask: &[u32],
        cyclic: bool,
    ) -> Result<(), Error> {
        let mask: String = mask.iter().rev().map(|w| format!("{w:08x}")).collect();
        self.command(&format!(
            "OPEN {dev} {samples} {mask}{}",
            if cyclic { " CYCLIC" } else { "" }
        ))?;
        self.status(&format!("open {dev}"))?;
        Ok(())
    }

    pub fn close(&mut self, dev: &str) -> Result<(), Error> {
        self.command(&format!("CLOSE {dev}"))?;
        self.status(&format!("close {dev}"))?;
        Ok(())
    }

    /// Reads `bytes` bytes of samples from the open buffer of `dev`: the channel mask the server reports
    /// (`mask_words` words, least significant first) and the interleaved data. `progress` gets the number of
    /// bytes received so far.
    pub fn read_buf(
        &mut self,
        dev: &str,
        bytes: usize,
        mask_words: usize,
        mut progress: impl FnMut(usize),
    ) -> Result<(Vec<u32>, Vec<u8>), Error> {
        let what = format!("read buffer of {dev}");
        self.command(&format!("READBUF {dev} {bytes}"))?;
        let mut data = vec![0u8; bytes];
        let mut mask = None;
        let mut got = 0;
        while got < bytes {
            let chunk = self.status(&what)?;
            if chunk == 0 {
                break;
            }
            if mask.is_none() {
                mask = Some(self.mask(mask_words)?);
            }
            if chunk > bytes - got {
                return Err(Error::Protocol(format!(
                    "{what}: server sent {chunk} bytes, {} were left",
                    bytes - got
                )));
            }
            self.stream.read_exact(&mut data[got..got + chunk])?;
            got += chunk;
            progress(got);
        }
        data.truncate(got);
        Ok((mask.unwrap_or_default(), data))
    }

    /// Writes samples to the open buffer of `dev`.
    pub fn write_buf(&mut self, dev: &str, data: &[u8]) -> Result<(), Error> {
        let what = format!("write buffer of {dev}");
        self.command(&format!("WRITEBUF {dev} {}", data.len()))?;
        self.status(&what)?;
        self.send(data)?;
        let written = self.status(&what)?;
        if written != data.len() {
            return Err(Error::Protocol(format!(
                "{what}: {written} of {} bytes taken",
                data.len()
            )));
        }
        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        let stream = self.stream.get_mut();
        stream.write_all(data)?;
        stream.flush()?;
        Ok(())
    }

    fn command(&mut self, line: &str) -> Result<(), Error> {
        self.send(format!("{line}\r\n").as_bytes())
    }

    /// The next non-empty line, without its line end.
    fn line(&mut self) -> Result<String, Error> {
        loop {
            let mut line = vec![];
            if self.stream.read_until(b'\n', &mut line)? == 0 {
                return Err(Error::Protocol("server closed the connection".into()));
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
            if !text.is_empty() {
                return Ok(text.to_string());
            }
        }
    }

    /// A status line: a count, or a negative errno turned into an error naming `what` was tried.
    fn status(&mut self, what: &str) -> Result<usize, Error> {
        let line = self.line()?;
        let code: i64 = line.parse().map_err(|_| {
            Error::Protocol(format!("{what}: expected a status line, got `{line}`"))
        })?;
        usize::try_from(code).map_err(|_| Error::Errno {
            code: code.unsigned_abs().min(i32::MAX as u64) as i32,
            what: what.to_string(),
        })
    }

    /// `len` payload bytes; the line end after them is left for [`line`](Self::line) to skip.
    fn payload(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        let mut data = vec![0u8; len];
        self.stream.read_exact(&mut data)?;
        Ok(data)
    }

    fn mask(&mut self, words: usize) -> Result<Vec<u32>, Error> {
        let line = self.line()?;
        if line.len() != words * 8 {
            return Err(Error::Protocol(format!(
                "channel mask `{line}` is not {words} words long"
            )));
        }
        let mut mask = (0..words)
            .map(|i| u32::from_str_radix(&line[i * 8..i * 8 + 8], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Error::Protocol(format!("channel mask `{line}` is not hex")))?;
        mask.reverse();
        Ok(mask)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // tells the server to close this client's buffers right away; without it, it does so when the socket ends
        let _ = self.send(b"EXIT\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A server that checks each expected command and sends the scripted answer.
    fn serve(script: Vec<(&'static [u8], &'static [u8])>) -> Client {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            for (expect, answer) in script {
                let mut got = vec![0u8; expect.len()];
                sock.read_exact(&mut got).unwrap();
                assert_eq!(
                    String::from_utf8_lossy(&got),
                    String::from_utf8_lossy(expect)
                );
                sock.write_all(answer).unwrap();
            }
        });
        Client::connect(addr, Duration::from_secs(2)).unwrap()
    }

    #[test]
    fn reads_and_writes_attributes() {
        let mut c = serve(vec![
            (b"VERSION\r\n", b"0.25.g1234567\n"),
            (
                b"READ iio:device0 sampling_frequency\r\n",
                b"10\n100000000\0\n",
            ),
            (
                b"READ iio:device0 INPUT voltage0 calibbias\r\n",
                b"4\n2048\n",
            ),
            (b"READ iio:device0 DEBUG direct_reg_access\r\n", b"-22\n"),
            (b"WRITE iio:device2 OUTPUT voltage0 raw 5\r\n2048\0", b"5\n"),
            (b"WRITE iio:device0 BUFFER watermark 2\r\n1\0", b"-13\n"),
        ]);
        assert_eq!(c.version().unwrap(), "0.25.g1234567");
        assert_eq!(
            c.read_attr("iio:device0", Attr::Device("sampling_frequency"))
                .unwrap(),
            "100000000"
        );
        let calibbias = Attr::Channel {
            channel: "voltage0",
            output: false,
            attr: "calibbias",
        };
        assert_eq!(c.read_attr("iio:device0", calibbias).unwrap(), "2048");
        let err = c
            .read_attr("iio:device0", Attr::Debug("direct_reg_access"))
            .unwrap_err();
        assert!(matches!(err, Error::Errno { code: 22, .. }), "{err}");
        let raw = Attr::Channel {
            channel: "voltage0",
            output: true,
            attr: "raw",
        };
        c.write_attr("iio:device2", raw, "2048").unwrap();
        let err = c
            .write_attr("iio:device0", Attr::Buffer("watermark"), "1")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "write iio:device0 buffer:watermark = 1: permission denied (EACCES, -13)"
        );
    }

    #[test]
    fn reads_buffer_in_chunks() {
        let mut c = serve(vec![
            (b"SET iio:device0 BUFFERS_COUNT 1\r\n", b"0\n"),
            (b"OPEN iio:device0 2 00000003\r\n", b"0\n"),
            (b"READBUF iio:device0 8\r\n", b"6\n00000003\nabcdef2\ngh"),
            (b"CLOSE iio:device0\r\n", b"0\n"),
        ]);
        c.set_buffers_count("iio:device0", 1).unwrap();
        c.open("iio:device0", 2, &[3], false).unwrap();
        let mut seen = vec![];
        let (mask, data) = c.read_buf("iio:device0", 8, 1, |n| seen.push(n)).unwrap();
        assert_eq!(mask, [3]);
        assert_eq!(data, b"abcdefgh");
        assert_eq!(seen, [6, 8]);
        c.close("iio:device0").unwrap();
    }

    #[test]
    fn writes_cyclic_buffer() {
        let mut c = serve(vec![
            (b"OPEN iio:device6 2 0000000100000000 CYCLIC\r\n", b"0\n"),
            (b"WRITEBUF iio:device6 4\r\n", b"4\n"),
            (b"\x01\x02\x03\x04", b"4\n"),
            (b"READBUF iio:device6 4\r\n", b"-110\n"),
        ]);
        c.open("iio:device6", 2, &[0, 1], true).unwrap();
        c.write_buf("iio:device6", &[1, 2, 3, 4]).unwrap();
        let err = c.read_buf("iio:device6", 4, 2, |_| {}).unwrap_err();
        assert!(err.to_string().contains("ETIMEDOUT"), "{err}");
    }

    #[test]
    fn reads_context() {
        let mut c = serve(vec![(
            b"PRINT\r\n",
            b"54\n<context name=\"x\"><device id=\"iio:device0\"/></context>\n",
        )]);
        let ctx = c.context().unwrap();
        assert_eq!(ctx.name, "x");
        assert_eq!(ctx.devices[0].id, "iio:device0");
    }
}
