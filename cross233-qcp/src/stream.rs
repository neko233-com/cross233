//! Adapter turning a message-oriented [`Conn`] into a stream-oriented
//! `AsyncRead + AsyncWrite` so it can be plugged into `tokio::io::copy_bidirectional`
//! for tunneling.
//!
//! A background task owns the `Conn` and bridges it to two unbounded channels:
//! inbound messages are pushed to the reader side; outbound writes (buffered and
//! flushed, or flushed once they exceed 64 KiB) are pulled and sent as QCP
//! messages. This keeps the stream API non-blocking while the window/flow-control
//! lives entirely inside `Conn`.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use crate::{Conn, QcpError, Stream};

const FLUSH_THRESHOLD: usize = 64 * 1024;
const SCRATCH: usize = 1 << 20;

/// Stream view over a [`Conn`].
pub struct QcpStream {
    reader: mpsc::UnboundedReceiver<Vec<u8>>,
    writer: mpsc::UnboundedSender<Vec<u8>>,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    conn: Conn,
    closed: bool,
    _task: tokio::task::JoinHandle<()>,
}

impl QcpStream {
    /// Spawn the bridge task and return a stream handle.
    pub fn new(conn: Conn, stream: Stream) -> Self {
        let (wt_tx, mut wt_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (rd_tx, rd_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let task_conn = conn.clone();
        let task = tokio::spawn(async move {
            let mut scratch = vec![0u8; SCRATCH];
            loop {
                tokio::select! {
                    // Inbound QCP message -> reader channel.
                    r = task_conn.recv(&mut scratch, Duration::from_secs(30)) => {
                        match r {
                            Ok((n, _)) if n > 0 => {
                                let _ = rd_tx.send(scratch[..n].to_vec());
                            }
                            Ok(_) | Err(QcpError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                    // Outbound write -> QCP send.
                    w = wt_rx.recv() => {
                        match w {
                            Some(b) => {
                                if task_conn.send_stream(&b, stream).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });
        QcpStream {
            reader: rd_rx,
            writer: wt_tx,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            conn,
            closed: false,
            _task: task,
        }
    }
}

impl AsyncRead for QcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let me = self.get_mut();
        if me.read_buf.is_empty() {
            match me.reader.poll_recv(cx) {
                Poll::Ready(Some(data)) => me.read_buf = data,
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF
                Poll::Pending => return Poll::Pending,
            }
        }
        let n = std::cmp::min(buf.remaining(), me.read_buf.len());
        buf.put_slice(&me.read_buf[..n]);
        me.read_buf.drain(..n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for QcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let me = self.get_mut();
        me.write_buf.extend_from_slice(buf);
        if me.write_buf.len() >= FLUSH_THRESHOLD {
            let data = std::mem::take(&mut me.write_buf);
            // Unbounded channel: send only fails if the receiver is gone (bridge
            // task ended).
            if me.writer.send(data).is_err() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "qcp stream closed",
                )));
            }
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let me = self.get_mut();
        if me.write_buf.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let data = std::mem::take(&mut me.write_buf);
        if me.writer.send(data).is_err() {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "qcp stream closed",
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().poll_flush(cx) {
            Poll::Ready(Ok(())) => {}
            other => return other,
        }
        let me = self.as_mut().get_mut();
        if !me.closed {
            me.closed = true;
            let conn = me.conn.clone();
            tokio::spawn(async move {
                conn.close().await;
            });
        }
        Poll::Ready(Ok(()))
    }
}

/// Convenience: turn a `Conn` into a stream of the given class.
pub fn into_stream(conn: Conn, stream: Stream) -> QcpStream {
    QcpStream::new(conn, stream)
}
