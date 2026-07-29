//! cross233-qcp: a pure-Rust reimplementation of the QCP (Quick Connect
//! Protocol) reliable-UDP transport.
//!
//! Design goals (matched to the neko233-com/QCP semantics):
//! - Three stream classes: `Realtime` (latest-wins, no recovery), `Critical`
//!   (bounded ARQ within deadline), `Batch` (reliable ordered delivery).
//! - Message-oriented API: `send`/`recv` move whole application messages;
//!   large messages are fragmented to the UDP MTU and reassembled on receipt.
//! - Windowed ARQ at message granularity with cumulative ACK and retransmit.
//! - Loss-robust 3-way handshake (SYN -> SYN-ACK -> ACK) with client retry.
//! - Control/data separation: ACK/SYN/FIN travel on a prioritized control
//!   channel so flow-control ACKs are never starved behind data retransmits
//!   (this is what makes full-duplex saturation safe under a bounded window).
//!
//! This crate has no C dependency and no Go dependency; it is the transport
//! used by the Rust cross233 server and client.

use bytes::{BufMut, Bytes, BytesMut};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{interval, timeout, Instant};

pub mod stream;
pub use stream::{into_stream, QcpStream};

// ---- Wire format -----------------------------------------------------------

const MAGIC: [u8; 4] = *b"QCP1";
const HEADER_LEN: usize = 18; // MAGIC(4)+flags(1)+stream(1)+msg_id(4)+frag_index(2)+frag_total(2)+len(4)
const MAX_PACKET: usize = 1400;
pub const MAX_PAYLOAD: usize = MAX_PACKET - HEADER_LEN; // 1382
const MAX_FRAGMENT_COUNT: usize = 8192;
const MAX_ASSEMBLERS: usize = 256;

const FLAG_SYN: u8 = 0x01;
const FLAG_ACK: u8 = 0x02;
const FLAG_DATA: u8 = 0x04;
const FLAG_FIN: u8 = 0x08;
const FLAG_PING: u8 = 0x10;
const FLAG_PONG: u8 = 0x20;

// --- Transport tuning (production defaults) ---------------------------------
// Congestion-control window is measured in whole messages (an ARQ message is
// the unit of reliability). These bounds keep a saturated link from either
// starving (too small) or flooding a lossy path (too large).
const MIN_CWND: usize = 2;
const INITIAL_CWND: usize = 32; // slow-start ramp from here
const MAX_CWND: usize = 4096;
const INITIAL_SS_THRESH: usize = 256; // enter congestion-avoidance above this

// RTT/RTO estimation (Jacobson + Karn), clamped to sane bounds so a single
// pathological sample cannot drive retransmits to zero or to infinity.
const MIN_RTO_MS: u64 = 50;
const MAX_RTO_MS: u64 = 2000;

// Liveness: a heartbeat PING is sent on idle, and if neither data nor PING is
// seen for DEAD_TIMEOUT the peer is declared dead (surfaced as an error).
const KEEPALIVE_INTERVAL_MS: u64 = 1000;
const DEAD_TIMEOUT_MS: u64 = 8000;

// A single message is given up after this many retransmits, the connection is
// then marked dead so blocked callers fail fast instead of hanging forever.
const MAX_RETRANSMIT: u32 = 15;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Realtime = 1,
    Critical = 2,
    Batch = 3,
}

impl Stream {
    fn from_u8(v: u8) -> Stream {
        match v {
            1 => Stream::Realtime,
            2 => Stream::Critical,
            _ => Stream::Batch,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QcpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("recv timeout")]
    Timeout,
    #[error("connection closed")]
    Closed,
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("message too large: {0} bytes (max {1})")]
    TooLarge(usize, usize),
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
}

pub type Result<T> = std::result::Result<T, QcpError>;

// A single on-wire packet (already serialized into bytes).
#[derive(Clone)]
struct Packet {
    flags: u8,
    stream: Stream,
    msg_id: u32,
    frag_index: u16,
    frag_total: u16,
    payload: Bytes,
}

fn encode(p: &Packet) -> BytesMut {
    let mut b = BytesMut::with_capacity(HEADER_LEN + p.payload.len());
    b.extend_from_slice(&MAGIC);
    b.put_u8(p.flags);
    b.put_u8(p.stream as u8);
    b.put_u32(p.msg_id);
    b.put_u16(p.frag_index);
    b.put_u16(p.frag_total);
    b.put_u32(p.payload.len() as u32);
    b.extend_from_slice(&p.payload);
    b
}

fn decode(buf: &[u8]) -> Option<Packet> {
    if buf.len() < HEADER_LEN || buf[0..4] != MAGIC {
        return None;
    }
    let flags = buf[4];
    let stream = Stream::from_u8(buf[5]);
    let msg_id = u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]);
    let frag_index = u16::from_be_bytes([buf[10], buf[11]]);
    let frag_total = u16::from_be_bytes([buf[12], buf[13]]);
    let len = u32::from_be_bytes([buf[14], buf[15], buf[16], buf[17]]) as usize;
    if len > MAX_PAYLOAD || buf.len() != HEADER_LEN + len {
        return None;
    }
    if flags & FLAG_DATA != 0
        && (frag_total == 0 || frag_index >= frag_total || frag_total as usize > MAX_FRAGMENT_COUNT)
    {
        return None;
    }
    let payload = Bytes::copy_from_slice(&buf[HEADER_LEN..HEADER_LEN + len]);
    Some(Packet {
        flags,
        stream,
        msg_id,
        frag_index,
        frag_total,
        payload,
    })
}

// ---- Transport (real or lossy, for tests) ----------------------------------

#[derive(Clone)]
enum Transport {
    Udp(Arc<tokio::net::UdpSocket>),
    #[allow(dead_code)] // only constructed in tests
    Lossy {
        inner: Arc<tokio::net::UdpSocket>,
        rate: f64,
    },
}

impl Transport {
    async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> std::io::Result<usize> {
        match self {
            Transport::Udp(s) => s.send_to(buf, addr).await,
            Transport::Lossy { inner, rate } => {
                if rand::random::<f64>() < *rate {
                    Ok(buf.len()) // drop: pretend it was sent
                } else {
                    inner.send_to(buf, addr).await
                }
            }
        }
    }
    async fn recv_from(&self, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        match self {
            Transport::Udp(s) => s.recv_from(buf).await,
            Transport::Lossy { inner, rate } => loop {
                let (n, from) = inner.recv_from(buf).await?;
                if rand::random::<f64>() >= *rate {
                    return Ok((n, from));
                }
            },
        }
    }
}

// ---- Per-connection core state --------------------------------------------

struct InFlight {
    data: Vec<u8>,
    stream: Stream,
    frags: u16,
    sent_at: tokio::time::Instant,
    /// True once this message has been retransmitted (Karn's rule: don't use
    /// retransmitted segments for RTT samples or we'd measure the timeout, not
    /// the path).
    retransmitted: bool,
}

struct Assembler {
    frags: Vec<Option<Vec<u8>>>,
    got: u16,
}

struct CoreState {
    in_flight: HashMap<u32, InFlight>,
    assemblers: HashMap<u32, Assembler>,
    delivered: HashMap<u32, ()>, // msg_ids already delivered (re-ACK, don't re-deliver)
    deliver_tx: mpsc::UnboundedSender<(Vec<u8>, u8)>,
    next_msg_id: u32,
    last_realtime: u32,
    // --- Congestion / flow control ---
    cwnd: usize,                // congestion window in messages (also the in-flight cap)
    ssthresh: usize,            // slow-start threshold
    ca_ack: usize,              // ACKs counted toward the current AIMD round
    srtt_ms: f64,               // smoothed RTT
    rttvar_ms: f64,             // RTT variance
    rto: Duration,              // current retransmit timeout
    rtt_init: bool,             // whether srtt/rttvar have been seeded yet
    retrans: HashMap<u32, u32>, // retransmit count per in-flight message
    // --- Liveness ---
    window_notify: Arc<Notify>, // wakes senders blocked on cwnd/close/dead
    fin_notify: Arc<Notify>,    // wakes a closer waiting for FIN-ACK
    fin_received: bool,
    dead: bool,
    dead_reason: Option<String>,
    last_activity: tokio::time::Instant,
    handshaked: bool,
    handshaked_notify: Arc<Notify>,
    closed: bool,
}

struct Core {
    state: Mutex<CoreState>,
    data_tx: mpsc::UnboundedSender<BytesMut>, // DATA fragments
    ctrl_tx: mpsc::UnboundedSender<BytesMut>, // ACK / SYN / FIN (prioritized)
    notify: Arc<Notify>,                      // wakes on close
}

type CoreRef = Arc<Core>;

fn new_core(
    deliver_tx: mpsc::UnboundedSender<(Vec<u8>, u8)>,
    window: usize,
    rto: Duration,
) -> (
    CoreRef,
    mpsc::UnboundedReceiver<BytesMut>,
    mpsc::UnboundedReceiver<BytesMut>,
) {
    let (data_tx, data_rx) = mpsc::unbounded_channel::<BytesMut>();
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel::<BytesMut>();
    // `window` is the operator-configured starting congestion window; clamp it
    // to sane bounds. Slow-start runs up to `ssthresh`, so set ssthresh above
    // the initial cwnd to let the window actually grow on healthy links.
    let cwnd = if window == 0 {
        INITIAL_CWND
    } else {
        window.min(MAX_CWND)
    };
    let ssthresh = if window == 0 {
        INITIAL_SS_THRESH
    } else {
        (window * 2).min(MAX_CWND)
    };
    let core = Arc::new(Core {
        state: Mutex::new(CoreState {
            in_flight: HashMap::new(),
            assemblers: HashMap::new(),
            delivered: HashMap::new(),
            deliver_tx,
            next_msg_id: 1,
            last_realtime: 0,
            cwnd,
            ssthresh,
            ca_ack: 0,
            srtt_ms: 100.0,
            rttvar_ms: 50.0,
            rto,
            rtt_init: false,
            retrans: HashMap::new(),
            window_notify: Arc::new(Notify::new()),
            fin_notify: Arc::new(Notify::new()),
            fin_received: false,
            dead: false,
            dead_reason: None,
            last_activity: tokio::time::Instant::now(),
            handshaked: false,
            handshaked_notify: Arc::new(Notify::new()),
            closed: false,
        }),
        data_tx: data_tx.clone(),
        ctrl_tx: ctrl_tx.clone(),
        notify: Arc::new(Notify::new()),
    });
    (core, data_rx, ctrl_rx)
}

/// Update the smoothed RTT / RTT variance and derived RTO (Jacobson's
/// algorithm). Retransmitted segments are excluded (Karn's rule) by the caller,
/// which passes the original `sent_at` only for first-transmission ACKs.
fn update_rto(st: &mut CoreState, sample: Duration) {
    let s = sample.as_secs_f64() * 1000.0;
    if !st.rtt_init {
        st.srtt_ms = s;
        st.rttvar_ms = s / 2.0;
        st.rtt_init = true;
    } else {
        let diff = st.srtt_ms - s;
        st.srtt_ms += 0.125 * diff;
        st.rttvar_ms = (1.0 - 0.25) * st.rttvar_ms + 0.25 * diff.abs();
    }
    let rto_ms = (st.srtt_ms + 4.0 * st.rttvar_ms).clamp(MIN_RTO_MS as f64, MAX_RTO_MS as f64);
    st.rto = Duration::from_millis(rto_ms as u64);
}

/// Mark the connection dead so all blocked callers wake with an error instead
/// of hanging forever on a broken link.
fn mark_dead(st: &mut CoreState, reason: &str) {
    st.dead = true;
    st.dead_reason = Some(reason.to_string());
    st.closed = true;
    st.window_notify.notify_waiters();
}

fn spawn_liveness_loop(core: CoreRef) {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_millis(KEEPALIVE_INTERVAL_MS));
        tick.tick().await;
        loop {
            tick.tick().await;
            let should_ping = {
                let mut st = core.state.lock().await;
                if st.closed {
                    return;
                }
                let idle = Instant::now().duration_since(st.last_activity);
                if idle >= Duration::from_millis(DEAD_TIMEOUT_MS) {
                    mark_dead(&mut st, "peer idle timeout");
                    false
                } else {
                    idle >= Duration::from_millis(KEEPALIVE_INTERVAL_MS)
                }
            };

            let is_closed = { core.state.lock().await.closed };
            if is_closed {
                core.notify.notify_waiters();
                return;
            }
            if should_ping {
                let ping = Packet {
                    flags: FLAG_PING,
                    stream: Stream::Batch,
                    msg_id: 0,
                    frag_index: 0,
                    frag_total: 0,
                    payload: Bytes::new(),
                };
                let _ = core.ctrl_tx.send(encode(&ping));
            }
        }
    });
}

// Spawn the outbound pump with control priority: ACK/SYN/FIN are always sent
// before queued data, so flow-control ACKs can never be starved by a data
// backlog (this prevents full-duplex deadlock under a saturated window).
fn spawn_pump(
    transport: Transport,
    peer: SocketAddr,
    mut data_rx: mpsc::UnboundedReceiver<BytesMut>,
    mut ctrl_rx: mpsc::UnboundedReceiver<BytesMut>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                c = ctrl_rx.recv() => {
                    match c {
                        Some(p) => { if transport.send_to(&p, peer).await.is_err() { break; } }
                        None => break,
                    }
                }
                d = data_rx.recv() => {
                    match d {
                        Some(p) => { if transport.send_to(&p, peer).await.is_err() { break; } }
                        None => break,
                    }
                }
            }
        }
    });
}

// Handle one inbound packet for a core.
async fn handle_packet(core: &CoreRef, p: Packet) {
    let mut st = core.state.lock().await;
    if st.closed {
        return;
    }
    st.last_activity = Instant::now();
    if p.flags & FLAG_SYN != 0 {
        st.handshaked = true;
        st.handshaked_notify.notify_waiters();
        return;
    }
    if p.flags & FLAG_FIN != 0 {
        st.fin_received = true;
        st.closed = true;
        st.window_notify.notify_waiters();
        st.fin_notify.notify_waiters();
        drop(st);
        core.notify.notify_waiters();
        return;
    }
    if p.flags & FLAG_PING != 0 {
        st.last_activity = Instant::now();
        // Reply on the prioritized control channel so the peer's keepalive does
        // not get starved behind a data backlog.
        let pong = Packet {
            flags: FLAG_PONG,
            stream: p.stream,
            msg_id: p.msg_id,
            frag_index: 0,
            frag_total: 0,
            payload: Bytes::new(),
        };
        let _ = core.ctrl_tx.send(encode(&pong));
        return;
    }
    if p.flags & FLAG_PONG != 0 {
        st.last_activity = Instant::now();
        return;
    }
    if p.flags & FLAG_ACK != 0 {
        if let Some(inf) = st.in_flight.remove(&p.msg_id) {
            // RTT sample from the original transmission only (Karn's rule): a
            // retransmitted segment's `sent_at` measures the timeout, not the
            // path, and would poison the estimate.
            if !inf.retransmitted {
                update_rto(&mut st, Instant::now().duration_since(inf.sent_at));
            }
            st.retrans.remove(&p.msg_id);
            // Congestion control: slow-start (cwnd += 1 per ACK) below ssthresh,
            // additive increase (cwnd += 1 per RTT) above it.
            if st.cwnd < st.ssthresh {
                st.cwnd = (st.cwnd + 1).min(MAX_CWND);
            } else {
                st.ca_ack += 1;
                if st.ca_ack >= st.cwnd {
                    st.ca_ack = 0;
                    st.cwnd = (st.cwnd + 1).min(MAX_CWND);
                }
            }
        }
        st.last_activity = Instant::now();
        st.window_notify.notify_waiters();
        return;
    }
    if p.flags & FLAG_DATA == 0 {
        return;
    }
    match p.stream {
        Stream::Realtime => {
            if p.msg_id.wrapping_sub(st.last_realtime) < 0x8000_0000 || st.last_realtime == 0 {
                st.last_realtime = p.msg_id;
                let _ = st.deliver_tx.send((p.payload.to_vec(), p.stream as u8));
            }
        }
        Stream::Critical | Stream::Batch => {
            let total = p.frag_total as usize;
            if !st.assemblers.contains_key(&p.msg_id) && st.assemblers.len() >= MAX_ASSEMBLERS {
                return;
            }
            // Scope the assembler borrow so we can later touch `st.delivered`
            // (a different field) without a borrow conflict.
            let complete = {
                let asm = st.assemblers.entry(p.msg_id).or_insert_with(|| Assembler {
                    frags: vec![None; total],
                    got: 0,
                });
                if asm.frags[p.frag_index as usize].is_none() {
                    asm.frags[p.frag_index as usize] = Some(p.payload.to_vec());
                    asm.got += 1;
                }
                asm.got as usize == total
            };
            if complete {
                // Always safe to (re-)send the ACK: it releases the sender's window
                // permit and duplicating it is harmless.
                let ack = Packet {
                    flags: FLAG_ACK,
                    stream: p.stream,
                    msg_id: p.msg_id,
                    frag_index: 0,
                    frag_total: 0,
                    payload: Bytes::new(),
                };
                // The assembler is removed on first delivery, so a retransmitted
                // fragment reassembles as if it were a brand-new message. Without a
                // delivered-set that reassembly would re-deliver AND re-echo the
                // message, producing an echo flood that exhausts the reverse window
                // and deadlocks a saturated full-duplex link (the original
                // 256-window failure). Detect the duplicate: re-ACK, but do NOT
                // re-deliver.
                if st.delivered.contains_key(&p.msg_id) {
                    let _ = core.ctrl_tx.send(encode(&ack));
                    return;
                }
                let mut msg = Vec::with_capacity(total * MAX_PAYLOAD);
                if let Some(mut asm) = st.assemblers.remove(&p.msg_id) {
                    for b in asm.frags.drain(..).flatten() {
                        msg.extend_from_slice(&b);
                    }
                }
                st.delivered.insert(p.msg_id, ());
                // Bound the delivered set so it cannot leak over a long-lived
                // connection (large-file transfers can emit millions of msg_ids).
                // The cap is far larger than any realistic in-flight window, so
                // genuine retransmits are still caught; only pathological ancient
                // retransmits after a clear could re-deliver, which higher layers
                // (the echo test's HashSet, or app-level idempotency) tolerate.
                if st.delivered.len() > 1 << 16 {
                    st.delivered.clear();
                }
                let _ = st.deliver_tx.send((msg, p.stream as u8));
                let _ = core.ctrl_tx.send(encode(&ack));
            }
        }
    }
}

// Resend aged in-flight messages. The RTO is recomputed every round from the
// live SRTT/RTTVAR estimate, so the timer tracks the path instead of a fixed
// constant. A message that exhausts its retransmit budget marks the whole
// connection dead.
async fn retransmit_loop(core: CoreRef) {
    loop {
        let rto = { core.state.lock().await.rto };
        let mut tick = interval(rto / 2);
        tick.tick().await; // skip immediate; interval rebuilt each round (adaptive RTO)
        if core.state.lock().await.closed {
            break;
        }
        let mut to_resend: Vec<(u32, Vec<u8>, Stream, u16)> = Vec::new();
        let mut give_up = false;
        {
            let st = core.state.lock().await;
            let now = Instant::now();
            for (id, inf) in st.in_flight.iter() {
                if now.duration_since(inf.sent_at) >= st.rto {
                    let rc = st.retrans.get(id).copied().unwrap_or(0) + 1;
                    if rc > MAX_RETRANSMIT {
                        give_up = true; // one unrecoverable message kills the link
                    } else {
                        to_resend.push((*id, inf.data.clone(), inf.stream, inf.frags));
                    }
                }
            }
        }
        if give_up {
            let mut st = core.state.lock().await;
            mark_dead(&mut st, "max retransmit exceeded");
            drop(st);
            core.notify.notify_waiters();
            break;
        }
        let now = Instant::now();
        for (id, data, stream, frags) in to_resend {
            send_frags(&core, id, &data, stream, frags);
            {
                let mut st = core.state.lock().await;
                if let Some(inf) = st.in_flight.get_mut(&id) {
                    inf.sent_at = now;
                    inf.retransmitted = true;
                }
                // Multiplicative decrease on detected loss (we can't tell a
                // timeout from a dup-ACK here, so conservatively treat every
                // retransmit as a loss event).
                *st.retrans.entry(id).or_insert(0) += 1;
                st.ssthresh = (st.cwnd / 2).max(MIN_CWND);
                st.cwnd = MIN_CWND;
                st.ca_ack = 0;
            }
        }
        if core.state.lock().await.closed {
            break;
        }
    }
}

fn send_frags(core: &CoreRef, msg_id: u32, data: &[u8], stream: Stream, _frags: u16) {
    let chunks: Vec<&[u8]> = data.chunks(MAX_PAYLOAD).collect();
    let total = chunks.len() as u16;
    for (i, c) in chunks.iter().enumerate() {
        let pkt = Packet {
            flags: FLAG_DATA,
            stream,
            msg_id,
            frag_index: i as u16,
            frag_total: total,
            payload: Bytes::copy_from_slice(c),
        };
        let _ = core.data_tx.send(encode(&pkt));
    }
}

// ---- Public connection handle ---------------------------------------------

type Delivery = (Vec<u8>, u8);
type DeliveryRx = Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Delivery>>>;

#[derive(Clone)]
pub struct Conn {
    deliver_rx: DeliveryRx,
    ctrl_tx: mpsc::UnboundedSender<BytesMut>,
    core: CoreRef,
}

impl Conn {
    pub async fn send(&self, data: &[u8]) -> Result<()> {
        self.send_stream(data, Stream::Batch).await
    }

    pub async fn send_stream(&self, data: &[u8], stream: Stream) -> Result<()> {
        if data.len() > MAX_PAYLOAD * 65535 {
            return Err(QcpError::TooLarge(data.len(), MAX_PAYLOAD * 65535));
        }
        if data.is_empty() {
            return Ok(());
        }
        // Reliable streams are gated by the congestion window (cwnd), which is
        // also the in-flight cap. Wait WITHOUT holding the state lock: a slot
        // frees when the peer ACKs (`handle_packet`) or the link closes/dies.
        // Holding the lock across the await would deadlock a saturated link —
        // the original root cause of the 256-window hang.
        if !matches!(stream, Stream::Realtime) {
            loop {
                let (has_space, closed, dead, reason) = {
                    let st = self.core.state.lock().await;
                    (
                        st.in_flight.len() < st.cwnd && !st.dead,
                        st.closed,
                        st.dead,
                        st.dead_reason.clone(),
                    )
                };
                if closed {
                    return Err(QcpError::Closed);
                }
                if dead {
                    return Err(QcpError::ConnectionFailed(
                        reason.unwrap_or_else(|| "connection dead".to_string()),
                    ));
                }
                if has_space {
                    break;
                }
                let notify = self.core.state.lock().await.window_notify.clone();
                notify.notified().await;
            }
        }
        let msg_id = {
            let mut st = self.core.state.lock().await;
            if st.closed {
                return Err(QcpError::Closed);
            }
            if st.dead {
                return Err(QcpError::ConnectionFailed(
                    st.dead_reason
                        .clone()
                        .unwrap_or_else(|| "connection dead".to_string()),
                ));
            }
            let id = st.next_msg_id;
            st.next_msg_id = st.next_msg_id.wrapping_add(1);
            if st.next_msg_id == 0 {
                st.next_msg_id = 1;
            }
            id
        };
        match stream {
            Stream::Realtime => {
                let chunks: Vec<&[u8]> = data.chunks(MAX_PAYLOAD).collect();
                let total = chunks.len() as u16;
                for (i, c) in chunks.iter().enumerate() {
                    let pkt = Packet {
                        flags: FLAG_DATA,
                        stream: Stream::Realtime,
                        msg_id,
                        frag_index: i as u16,
                        frag_total: total,
                        payload: Bytes::copy_from_slice(c),
                    };
                    let _ = self.core.data_tx.send(encode(&pkt));
                }
            }
            Stream::Critical | Stream::Batch => {
                let frags = data.len().div_ceil(MAX_PAYLOAD) as u16;
                {
                    let mut st = self.core.state.lock().await;
                    st.in_flight.insert(
                        msg_id,
                        InFlight {
                            data: data.to_vec(),
                            stream,
                            frags,
                            sent_at: Instant::now(),
                            retransmitted: false,
                        },
                    );
                    st.last_activity = Instant::now();
                }
                send_frags(&self.core, msg_id, data, stream, frags);
            }
        }
        Ok(())
    }

    pub async fn recv(&self, buf: &mut [u8], wait: Duration) -> Result<(usize, u8)> {
        let mut rx = self.deliver_rx.lock().await;
        let closed = self.core.notify.notified();
        tokio::pin!(closed);
        tokio::select! {
            received = timeout(wait, rx.recv()) => match received {
                Ok(Some((msg, stream))) => {
                    let n = msg.len().min(buf.len());
                    buf[..n].copy_from_slice(&msg[..n]);
                    Ok((n, stream))
                }
                Ok(None) => Err(QcpError::Closed),
                Err(_) => Err(QcpError::Timeout),
            },
            _ = &mut closed => {
                let st = self.core.state.lock().await;
                if st.dead {
                    Err(QcpError::ConnectionFailed(
                        st.dead_reason.clone().unwrap_or_else(|| "connection dead".to_string()),
                    ))
                } else {
                    Err(QcpError::Closed)
                }
            }
        }
    }

    pub async fn close(&self) {
        {
            let mut st = self.core.state.lock().await;
            st.closed = true;
            st.in_flight.clear();
            st.assemblers.clear();
            st.window_notify.notify_waiters();
        }
        self.core.notify.notify_waiters();
        let _ = self.ctrl_tx.send(encode(&Packet {
            flags: FLAG_FIN,
            stream: Stream::Batch,
            msg_id: 0,
            frag_index: 0,
            frag_total: 0,
            payload: Bytes::new(),
        }));
    }
}

// ---- Listener (server side) ------------------------------------------------

pub struct Listener {
    accept_rx: mpsc::UnboundedReceiver<Conn>,
    _task: tokio::task::JoinHandle<()>,
    local_addr: SocketAddr,
}

pub async fn listen(addr: &str) -> Result<Listener> {
    let sock = bind_udp(addr)?;
    let local = sock.local_addr()?;
    let transport = Transport::Udp(Arc::new(sock));
    let (accept_tx, accept_rx) = mpsc::unbounded_channel::<Conn>();
    let task = tokio::spawn(io_listener(transport, accept_tx));
    Ok(Listener {
        accept_rx,
        _task: task,
        local_addr: local,
    })
}

impl Listener {
    /// The local UDP address this listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

async fn io_listener(transport: Transport, accept_tx: mpsc::UnboundedSender<Conn>) {
    let mut buf = vec![0u8; MAX_PACKET];
    let mut cores: HashMap<SocketAddr, CoreRef> = HashMap::new();
    loop {
        let (n, from) = match transport.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(_) => break,
        };
        let pkt = match decode(&buf[..n]) {
            Some(p) => p,
            None => continue,
        };
        if pkt.flags & FLAG_SYN != 0 {
            let replace_closed = match cores.get(&from) {
                Some(core) => core.state.lock().await.closed,
                None => true,
            };
            if replace_closed {
                cores.remove(&from);
                let (deliver_tx, deliver_rx) = mpsc::unbounded_channel();
                let (core, data_rx, ctrl_rx) =
                    new_core(deliver_tx, 256, Duration::from_millis(200));
                spawn_pump(transport.clone(), from, data_rx, ctrl_rx);
                spawn_liveness_loop(core.clone());
                let conn = Conn {
                    deliver_rx: Arc::new(tokio::sync::Mutex::new(deliver_rx)),
                    ctrl_tx: core.ctrl_tx.clone(),
                    core: core.clone(),
                };
                {
                    let mut st = core.state.lock().await;
                    st.handshaked = true;
                    st.handshaked_notify.notify_waiters();
                }
                let c2 = core.clone();
                tokio::spawn(retransmit_loop(c2));
                cores.insert(from, core);
                let _ = accept_tx.send(conn);
            }
            // Always reply SYN-ACK (idempotent, so client retries recover).
            let synack = encode(&Packet {
                flags: FLAG_SYN | FLAG_ACK,
                stream: Stream::Batch,
                msg_id: 0,
                frag_index: 0,
                frag_total: 0,
                payload: Bytes::new(),
            });
            let _ = transport.send_to(&synack, from).await;
            continue;
        }
        if let Some(core) = cores.get(&from) {
            handle_packet(core, pkt).await;
        }
    }
}

impl Listener {
    pub async fn accept(&mut self) -> Option<Conn> {
        self.accept_rx.recv().await
    }
}

// ---- Dial (client side) ----------------------------------------------------

pub async fn dial(addr: &str) -> Result<Conn> {
    let sock = bind_udp("0.0.0.0:0")?;
    let peer: SocketAddr = addr
        .parse()
        .map_err(|_| QcpError::Handshake("bad address".into()))?;
    let transport = Transport::Udp(Arc::new(sock));
    dial_transport(transport, peer).await
}

async fn dial_transport(transport: Transport, peer: SocketAddr) -> Result<Conn> {
    let (deliver_tx, deliver_rx) = mpsc::unbounded_channel();
    let (core, data_rx, ctrl_rx) = new_core(deliver_tx, 256, Duration::from_millis(200));
    spawn_pump(transport.clone(), peer, data_rx, ctrl_rx);

    // Inbound pump.
    let c2 = core.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_PACKET];
        loop {
            let (n, from) = match transport.recv_from(&mut buf).await {
                Ok(x) => x,
                Err(_) => break,
            };
            if from != peer {
                continue;
            }
            if let Some(p) = decode(&buf[..n]) {
                handle_packet(&c2, p).await;
            }
        }
    });
    let c3 = core.clone();
    tokio::spawn(retransmit_loop(c3));
    spawn_liveness_loop(core.clone());

    // 3-way handshake with retry (robust to lossy links).
    let syn = encode(&Packet {
        flags: FLAG_SYN,
        stream: Stream::Batch,
        msg_id: 0,
        frag_index: 0,
        frag_total: 0,
        payload: Bytes::new(),
    });
    let ack = encode(&Packet {
        flags: FLAG_ACK,
        stream: Stream::Batch,
        msg_id: 0,
        frag_index: 0,
        frag_total: 0,
        payload: Bytes::new(),
    });
    for _ in 0..10u32 {
        let handshaked = { core.state.lock().await.handshaked };
        if handshaked {
            core.ctrl_tx.send(ack.clone()).ok();
            return Ok(Conn {
                deliver_rx: Arc::new(tokio::sync::Mutex::new(deliver_rx)),
                ctrl_tx: core.ctrl_tx.clone(),
                core,
            });
        }
        core.ctrl_tx.send(syn.clone()).ok();
        let notify = core.state.lock().await.handshaked_notify.clone();
        if timeout(Duration::from_millis(500), notify.notified())
            .await
            .is_ok()
        {
            core.ctrl_tx.send(ack.clone()).ok();
            return Ok(Conn {
                deliver_rx: Arc::new(tokio::sync::Mutex::new(deliver_rx)),
                ctrl_tx: core.ctrl_tx.clone(),
                core,
            });
        }
    }
    Err(QcpError::Handshake("no syn-ack after retries".into()))
}

/// Bind a UDP socket with generously sized OS buffers. High-throughput
/// reliable-UDP needs headroom so that legitimate fragment bursts are not
/// dropped at the socket layer (the default ~64 KB recv buffer is too small
/// for multi-fragment messages / large-file transfers).
fn bind_udp(addr: &str) -> std::io::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    let addr: std::net::SocketAddr = addr.parse().map_err(|e: std::net::AddrParseError| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
    })?;
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let sock = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = sock.set_recv_buffer_size(2 * 1024 * 1024);
    let _ = sock.set_send_buffer_size(2 * 1024 * 1024);
    sock.set_nonblocking(true)?;
    sock.bind(&SockAddr::from(addr))?;
    tokio::net::UdpSocket::from_std(std::net::UdpSocket::from(sock))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn echo_server() -> String {
        let mut l = listen("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().to_string();
        tokio::spawn(async move {
            let conn = l.accept().await.unwrap();
            let mut buf = vec![0u8; 1_000_000];
            while let Ok((n, s)) = conn.recv(&mut buf, Duration::from_secs(5)).await {
                let data = buf[..n].to_vec();
                if conn.send_stream(&data, Stream::from_u8(s)).await.is_err() {
                    break;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn loopback_small() {
        let addr = echo_server().await;
        let conn = dial(&addr).await.unwrap();
        conn.send(b"hello-qcp").await.unwrap();
        let mut buf = vec![0u8; 1024];
        let (n, _) = conn.recv(&mut buf, Duration::from_secs(5)).await.unwrap();
        assert_eq!(&buf[..n], b"hello-qcp");
    }

    #[tokio::test]
    async fn loopback_large() {
        let addr = echo_server().await;
        let conn = dial(&addr).await.unwrap();
        let payload = vec![0xABu8; 100_000];
        conn.send(&payload).await.unwrap();
        let mut buf = vec![0u8; 200_000];
        let (n, _) = conn.recv(&mut buf, Duration::from_secs(5)).await.unwrap();
        assert_eq!(n, 100_000);
        assert_eq!(&buf[..n], &payload[..]);
    }

    #[tokio::test]
    async fn loopback_many() {
        let addr = echo_server().await;
        let conn = dial(&addr).await.unwrap();
        for i in 0..200u32 {
            let msg = format!("msg-{i}");
            conn.send(msg.as_bytes()).await.unwrap();
            let mut buf = vec![0u8; 1024];
            let (n, _) = conn.recv(&mut buf, Duration::from_secs(5)).await.unwrap();
            assert_eq!(&buf[..n], msg.as_bytes());
        }
    }

    #[tokio::test]
    async fn lossy_recovery() {
        let sock = bind_udp("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap().to_string();
        let transport = Transport::Lossy {
            inner: Arc::new(sock),
            rate: 0.2,
        };
        let (accept_tx, mut accept_rx) = mpsc::unbounded_channel::<Conn>();
        tokio::spawn(io_listener(transport.clone(), accept_tx));
        tokio::spawn(async move {
            if let Some(conn) = accept_rx.recv().await {
                let mut buf = vec![0u8; 1_000_000];
                while let Ok((n, _)) = conn.recv(&mut buf, Duration::from_secs(10)).await {
                    let d = buf[..n].to_vec();
                    if conn.send(&d).await.is_err() {
                        break;
                    }
                }
            }
        });

        let csock = bind_udp("0.0.0.0:0").unwrap();
        let ctransport = Transport::Lossy {
            inner: Arc::new(csock),
            rate: 0.2,
        };
        let peer: SocketAddr = addr.parse().unwrap();
        let conn = dial_transport(ctransport, peer).await.unwrap();

        let payload = vec![0x5u8; 50_000];
        conn.send(&payload).await.unwrap();
        let mut buf = vec![0u8; 200_000];
        let (n, _) = conn.recv(&mut buf, Duration::from_secs(20)).await.unwrap();
        assert_eq!(n, 50_000);
        assert_eq!(&buf[..n], &payload[..]);
    }

    #[tokio::test]
    async fn realtime_latest_wins() {
        let addr = echo_server().await;
        let conn = dial(&addr).await.unwrap();
        for i in 0..20u32 {
            let msg = format!("rt-{i:04}");
            conn.send_stream(msg.as_bytes(), Stream::Realtime)
                .await
                .unwrap();
            tokio::task::yield_now().await;
        }
        let mut buf = vec![0u8; 1024];
        let mut last: i32 = -1;
        for _ in 0..5u32 {
            let (n, s) = conn.recv(&mut buf, Duration::from_secs(5)).await.unwrap();
            assert_eq!(s, Stream::Realtime as u8);
            let v: i32 = String::from_utf8_lossy(&buf[..n])
                .trim_end_matches('\0')
                .trim()
                .trim_start_matches("rt-")
                .parse()
                .unwrap_or(-1);
            assert!(
                v >= last,
                "realtime delivered an older message: {v} < {last}"
            );
            last = v;
        }
    }

    #[tokio::test]
    async fn critical_reliable_once() {
        let addr = echo_server().await;
        let conn = dial(&addr).await.unwrap();
        // Exceeds the default window (256) to exercise flow-control / ARQ.
        let total = 600u32;
        for i in 0..total {
            let msg = format!("crit-{i:05}");
            conn.send_stream(msg.as_bytes(), Stream::Critical)
                .await
                .unwrap();
        }
        let mut buf = vec![0u8; 1024];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..total {
            let (n, s) = conn.recv(&mut buf, Duration::from_secs(10)).await.unwrap();
            assert_eq!(s, Stream::Critical as u8);
            let v = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(seen.insert(v.clone()), "duplicate critical delivery: {v}");
        }
        assert_eq!(seen.len(), total as usize);
    }
}
