use std::net::SocketAddr;

pub fn build_header(src: SocketAddr, dst: SocketAddr, version: Option<&str>) -> Vec<u8> {
    match version {
        Some("v2") => build_v2_header(src, dst),
        _ => build_v1_header(src, dst),
    }
}

fn build_v1_header(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let proto = match (src, dst) {
        (SocketAddr::V4(_), SocketAddr::V4(_)) => "TCP4",
        _ => "TCP6",
    };
    let src_ip = src.ip();
    let src_port = src.port();
    let dst_ip = dst.ip();
    let dst_port = dst.port();
    format!("PROXY {proto} {src_ip} {dst_ip} {src_port} {dst_port}\r\n").into_bytes()
}

fn build_v2_header(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let mut buf = Vec::with_capacity(52);
    buf.extend_from_slice(b"\x0D\x0A\x0D\x0A\x00\x0D\x0A\x51\x55\x49\x54\x0A");
    match (&src, &dst) {
        (SocketAddr::V4(s), SocketAddr::V4(d)) => {
            buf.push(0x21);
            buf.push(0x11);
            let len: u16 = 12;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(&s.ip().octets());
            buf.extend_from_slice(&d.ip().octets());
            buf.extend_from_slice(&s.port().to_be_bytes());
            buf.extend_from_slice(&d.port().to_be_bytes());
        }
        (SocketAddr::V6(s), SocketAddr::V6(d)) => {
            buf.push(0x21);
            buf.push(0x21);
            let len: u16 = 36;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(&s.ip().octets());
            buf.extend_from_slice(&d.ip().octets());
            buf.extend_from_slice(&s.port().to_be_bytes());
            buf.extend_from_slice(&d.port().to_be_bytes());
        }
        _ => {
            buf.push(0x20);
            buf.push(0x00);
            buf.extend_from_slice(&0u16.to_be_bytes());
        }
    }
    buf
}
