//! `--remote-debugging-pipe`: Chrome reads commands from fd 3 and writes
//! responses and events to fd 4, each message terminated by a NUL byte.

use std::io;
use std::os::fd::OwnedFd;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;
use tokio::process::{Child, Command};

use super::frame::FrameDecoder;
use super::{Transport, TransportReceiver, TransportSender};
use crate::error::TransportError;

mod fd;

/// Read chunk size. Large messages grow the buffer as needed.
const READ_CHUNK: usize = 64 * 1024;

/// A CDP transport over the two pipes of `--remote-debugging-pipe`.
#[derive(Debug)]
pub struct PipeTransport {
    to_chrome: pipe::Sender,
    from_chrome: pipe::Receiver,
}

impl PipeTransport {
    /// Spawns `cmd` with its fd 3 and fd 4 connected to a new transport.
    ///
    /// The caller supplies the program and its flags, which must include
    /// `--remote-debugging-pipe`. Everything else about the child (stdio,
    /// `kill_on_drop`, process group) is also up to the caller.
    ///
    /// Must be called inside a Tokio runtime.
    pub fn spawn(mut cmd: Command) -> io::Result<(Self, Child)> {
        // Parent writes, child reads on fd 3.
        let (child_reads, parent_writes) = io::pipe()?;
        // Child writes on fd 4, parent reads.
        let (parent_reads, child_writes) = io::pipe()?;
        let child_fds = fd::ChildFds::new(&child_reads.into(), &child_writes.into())?;
        fd::install(&mut cmd, &child_fds);
        let child = cmd.spawn()?;
        // Close our copies of the child's ends, so we see EOF when it exits.
        drop(child_fds);
        let transport = Self::from_fds(parent_writes.into(), parent_reads.into())?;
        Ok((transport, child))
    }

    /// Wraps existing pipe ends: `to_chrome` is written to (Chrome's fd 3),
    /// `from_chrome` is read from (Chrome's fd 4). Both must be pipes (FIFOs).
    ///
    /// Must be called inside a Tokio runtime.
    pub fn from_fds(to_chrome: OwnedFd, from_chrome: OwnedFd) -> io::Result<Self> {
        Ok(Self {
            to_chrome: pipe::Sender::from_owned_fd(to_chrome)?,
            from_chrome: pipe::Receiver::from_owned_fd(from_chrome)?,
        })
    }
}

/// Sending half of a [`PipeTransport`].
#[derive(Debug)]
pub struct PipeSender(Option<pipe::Sender>);

/// Receiving half of a [`PipeTransport`].
#[derive(Debug)]
pub struct PipeReceiver {
    pipe: pipe::Receiver,
    decoder: FrameDecoder,
}

impl Transport for PipeTransport {
    type Sender = PipeSender;
    type Receiver = PipeReceiver;

    fn split(self) -> (PipeSender, PipeReceiver) {
        (
            PipeSender(Some(self.to_chrome)),
            PipeReceiver {
                pipe: self.from_chrome,
                decoder: FrameDecoder::new(),
            },
        )
    }
}

impl TransportSender for PipeSender {
    async fn send(&mut self, msg: Bytes) -> Result<(), TransportError> {
        let pipe = self.0.as_mut().ok_or(TransportError::Closed)?;
        // Two writes instead of copying a possibly huge message to append 1 byte.
        pipe.write_all(&msg).await?;
        pipe.write_all(b"\0").await?;
        Ok(())
    }

    fn close(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send {
        // Dropping the write end gives Chrome EOF on fd 3.
        self.0 = None;
        std::future::ready(Ok(()))
    }
}

impl TransportReceiver for PipeReceiver {
    async fn recv(&mut self) -> Result<Option<Bytes>, TransportError> {
        loop {
            if let Some(frame) = self.decoder.next_frame() {
                return Ok(Some(frame));
            }
            let buf = self.decoder.buffer_mut();
            buf.reserve(READ_CHUNK);
            // `read_buf` is cancel safe; bytes read stay in the decoder.
            if self.pipe.read_buf(buf).await? == 0 {
                if self.decoder.pending() > 0 {
                    tracing::debug!(
                        bytes = self.decoder.pending(),
                        "pipe closed with an unterminated frame; dropping it"
                    );
                }
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use bytes::Bytes;
    use tokio::process::Command;

    use super::PipeTransport;
    use crate::transport::{Transport, TransportReceiver, TransportSender};

    /// A stand-in for Chrome that echoes fd 3 to fd 4, so the fd wiring is
    /// tested on every Unix CI runner, with or without Chrome.
    fn echo_child() -> Command {
        let mut cmd = Command::new("sh");
        // `exec`, so killing the child kills the process holding the pipes;
        // a forked `cat` would outlive `sh` and keep fd 4 open.
        cmd.args(["-c", "exec cat <&3 >&4"])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }

    #[tokio::test]
    async fn child_sees_fds_3_and_4() {
        let (transport, mut child) = PipeTransport::spawn(echo_child()).unwrap();
        let (mut tx, mut rx) = transport.split();
        tx.send(Bytes::from_static(b"{\"id\":1}")).await.unwrap();
        tx.send(Bytes::from_static(b"{\"id\":2}")).await.unwrap();
        assert_eq!(rx.recv().await.unwrap().unwrap(), &b"{\"id\":1}"[..]);
        assert_eq!(rx.recv().await.unwrap().unwrap(), &b"{\"id\":2}"[..]);
        // Closing our write end gives the child EOF; it exits and we see EOF.
        tx.close().await.unwrap();
        assert!(rx.recv().await.unwrap().is_none());
        assert!(child.wait().await.unwrap().success());
    }

    #[tokio::test]
    async fn large_messages_survive() {
        let (transport, _child) = PipeTransport::spawn(echo_child()).unwrap();
        let (mut tx, mut rx) = transport.split();
        let big = Bytes::from(vec![b'x'; 8 * 1024 * 1024]);
        let send = tokio::spawn(async move {
            tx.send(big).await.unwrap();
            tx
        });
        let got = rx.recv().await.unwrap().unwrap();
        assert_eq!(got.len(), 8 * 1024 * 1024);
        drop(send.await.unwrap());
    }

    #[tokio::test]
    async fn killed_child_gives_eof() {
        let (transport, mut child) = PipeTransport::spawn(echo_child()).unwrap();
        let (_tx, mut rx) = transport.split();
        child.kill().await.unwrap();
        assert!(rx.recv().await.unwrap().is_none());
    }
}
