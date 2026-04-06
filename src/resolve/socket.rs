use crate::error::AppError;
use mio::net::UdpSocket;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

pub(crate) const MAX_DNS_PACKET_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddressFamily {
    V4,
    V6,
}

#[derive(Debug)]
pub(crate) struct BoundSocket {
    pub(crate) socket: UdpSocket,
    pub(crate) family: AddressFamily,
}

#[derive(Debug, Clone)]
pub(crate) struct OutPacket {
    pub(crate) addr: SocketAddr,
    pub(crate) len: usize,
    pub(crate) buf: [u8; MAX_DNS_PACKET_SIZE],
}

#[derive(Debug, Clone)]
pub(crate) struct InPacket {
    pub(crate) addr: SocketAddr,
    pub(crate) len: usize,
    pub(crate) buf: [u8; MAX_DNS_PACKET_SIZE],
}

pub(crate) fn address_family(addr: SocketAddr) -> AddressFamily {
    if addr.is_ipv4() {
        AddressFamily::V4
    } else {
        AddressFamily::V6
    }
}

pub(crate) fn create_udp_sockets(
    resolvers: &[SocketAddr],
    socket_count: usize,
    recv_buf_size: usize,
    send_buf_size: usize,
) -> Result<Vec<BoundSocket>, AppError> {
    let mut sockets = Vec::new();
    let socket_count = socket_count.max(1);

    if resolvers.iter().any(|resolver| resolver.is_ipv4()) {
        for _ in 0..socket_count {
            sockets.push(BoundSocket {
                socket: create_socket(AddressFamily::V4, recv_buf_size, send_buf_size)?,
                family: AddressFamily::V4,
            });
        }
    }

    if resolvers.iter().any(|resolver| resolver.is_ipv6()) {
        for _ in 0..socket_count {
            sockets.push(BoundSocket {
                socket: create_socket(AddressFamily::V6, recv_buf_size, send_buf_size)?,
                family: AddressFamily::V6,
            });
        }
    }

    if sockets.is_empty() {
        return Err(AppError::new("no compatible DNS sockets could be created")
            .with_help("configure at least one valid IPv4 or IPv6 resolver address"));
    }

    Ok(sockets)
}

pub(crate) fn send_batch(socket: &UdpSocket, packets: &[OutPacket]) -> usize {
    let mut sent = 0usize;

    for packet in packets {
        match socket.send_to(&packet.buf[..packet.len], packet.addr) {
            Ok(_) => sent += 1,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }

    sent
}

pub(crate) fn recv_batch(socket: &UdpSocket, batch_size: usize, out: &mut Vec<InPacket>) -> usize {
    out.clear();

    while out.len() < batch_size {
        let mut packet = InPacket {
            addr: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            len: 0,
            buf: [0; MAX_DNS_PACKET_SIZE],
        };

        match socket.recv_from(&mut packet.buf) {
            Ok((len, addr)) => {
                packet.addr = addr;
                packet.len = len;
                out.push(packet);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }

    out.len()
}

fn create_socket(
    family: AddressFamily,
    recv_buf_size: usize,
    send_buf_size: usize,
) -> Result<UdpSocket, AppError> {
    let (domain, bind_addr) = match family {
        AddressFamily::V4 => (Domain::IPV4, SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))),
        AddressFamily::V6 => (Domain::IPV6, SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))),
    };

    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|err| AppError::io("failed to create DNS socket", err))?;
    socket
        .set_nonblocking(true)
        .map_err(|err| AppError::io("failed to make DNS socket non-blocking", err))?;
    let _ = socket.set_recv_buffer_size(recv_buf_size);
    let _ = socket.set_send_buffer_size(send_buf_size);
    socket
        .bind(&SockAddr::from(bind_addr))
        .map_err(|err| AppError::io("failed to bind DNS socket", err))?;

    let std_socket: std::net::UdpSocket = socket.into();
    Ok(UdpSocket::from_std(std_socket))
}
