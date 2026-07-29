use std::net::SocketAddr;

pub fn build_header(version: &str, src_addr: SocketAddr, dst_addr: SocketAddr) -> Vec<u8> {
    match version {
        "v1" | "" => build_v1_header(src_addr, dst_addr),
        "v2" => build_v2_header(src_addr, dst_addr),
        _ => build_v1_header(src_addr, dst_addr),
    }
}

fn build_v1_header(src_addr: SocketAddr, dst_addr: SocketAddr) -> Vec<u8> {
    let inet = match src_addr {
        SocketAddr::V4(_) => "TCP4",
        SocketAddr::V6(_) => "TCP6",
    };
    let header = format!(
        "PROXY {} {} {} {} {}\r\n",
        inet,
        src_addr.ip(),
        dst_addr.ip(),
        src_addr.port(),
        dst_addr.port()
    );
    header.into_bytes()
}

fn build_v2_header(src_addr: SocketAddr, dst_addr: SocketAddr) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(b"\x0D\x0A\x0D\x0A\x00\x0D\x0A\x51\x55\x49\x54\x0A");
    let (family, mut addr_bytes) = match (src_addr, dst_addr) {
        (SocketAddr::V4(s), SocketAddr::V4(d)) => {
            let mut b = Vec::with_capacity(12);
            b.extend_from_slice(&s.ip().octets());
            b.extend_from_slice(&s.port().to_be_bytes());
            b.extend_from_slice(&d.ip().octets());
            b.extend_from_slice(&d.port().to_be_bytes());
            (0x11u8, b)
        }
        (SocketAddr::V6(s), SocketAddr::V6(d)) => {
            let mut b = Vec::with_capacity(36);
            b.extend_from_slice(&s.ip().octets());
            b.extend_from_slice(&s.port().to_be_bytes());
            b.extend_from_slice(&d.ip().octets());
            b.extend_from_slice(&d.port().to_be_bytes());
            (0x21u8, b)
        }
        _ => {
            let mut b = Vec::with_capacity(12);
            if let SocketAddr::V4(s) = src_addr {
                b.extend_from_slice(&s.ip().octets());
                b.extend_from_slice(&s.port().to_be_bytes());
            } else {
                b.extend_from_slice(&[0u8; 6]);
            }
            if let SocketAddr::V4(d) = dst_addr {
                b.extend_from_slice(&d.ip().octets());
                b.extend_from_slice(&d.port().to_be_bytes());
            } else {
                b.extend_from_slice(&[0u8; 6]);
            }
            (0x11u8, b)
        }
    };
    buf.push(0x21);
    buf.push(family);
    buf.extend_from_slice(&(addr_bytes.len() as u16).to_be_bytes());
    buf.append(&mut addr_bytes);
    buf
}
