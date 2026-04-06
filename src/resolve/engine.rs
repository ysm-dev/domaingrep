use super::slab::{LookupSlab, LookupSlot};
use super::socket::{
    address_family, create_udp_sockets, recv_batch, send_batch, InPacket, OutPacket,
};
use super::wheel::TimingWheel;
use super::wire::{build_query, parse_response_header};
use super::{is_definitive, ResolveConfig, ResolveResponse, QTYPE_NS};
use crate::error::AppError;
use mio::{Events, Interest, Poll, Token};
use std::collections::VecDeque;
use std::time::Duration;

const WHEEL_BUCKET_COUNT: usize = 1024;
const WHEEL_RESOLUTION_MS: u64 = 10;

#[derive(Debug, Clone, Copy)]
struct QueuedLookup {
    domain_index: u32,
    attempt: u8,
}

#[derive(Debug)]
struct Engine {
    config: ResolveConfig,
    poll: Poll,
    events: Events,
    sockets: Vec<super::socket::BoundSocket>,
    resolvers: Vec<std::net::SocketAddr>,
    slab: LookupSlab,
    wheel: TimingWheel,
    pending: VecDeque<QueuedLookup>,
    send_batches: Vec<VecDeque<OutPacket>>,
    recv_packets: Vec<InPacket>,
    expired: Vec<u16>,
    results: Vec<Option<ResolveResponse>>,
    done: Vec<bool>,
    completed: usize,
    next_v4_socket: usize,
    next_v6_socket: usize,
}

pub(crate) fn resolve_raw_domains(
    config: &ResolveConfig,
    domains: &[String],
) -> Result<Vec<Option<ResolveResponse>>, AppError> {
    if domains.is_empty() {
        return Ok(Vec::new());
    }

    let mut engine = Engine::new(config.clone(), domains.len())?;
    engine.run(domains)
}

impl Engine {
    fn new(config: ResolveConfig, domain_count: usize) -> Result<Self, AppError> {
        if config.resolvers.is_empty() {
            return Err(AppError::new("no DNS resolvers configured")
                .with_help("configure at least one resolver via DOMAINGREP_RESOLVERS"));
        }

        let poll = Poll::new().map_err(|err| AppError::io("failed to create DNS poller", err))?;
        let mut sockets = create_udp_sockets(
            &config.resolvers,
            config.socket_count,
            config.recv_buf_size,
            config.send_buf_size,
        )?;
        let resolvers = config.resolvers.clone();
        let recv_batch_size = config.recv_batch_size;
        let concurrency = config.concurrency;

        for (index, socket) in sockets.iter_mut().enumerate() {
            poll.registry()
                .register(&mut socket.socket, Token(index), Interest::READABLE)
                .map_err(|err| AppError::io("failed to register DNS socket", err))?;
        }

        let mut pending = VecDeque::with_capacity(domain_count);
        for index in 0..domain_count {
            pending.push_back(QueuedLookup {
                domain_index: index as u32,
                attempt: 1,
            });
        }

        let mut send_batches = Vec::with_capacity(sockets.len());
        for _ in 0..sockets.len() {
            send_batches.push(VecDeque::with_capacity(config.send_batch_size));
        }

        Ok(Self {
            config,
            poll,
            events: Events::with_capacity(1024),
            sockets,
            resolvers,
            slab: LookupSlab::new(),
            wheel: TimingWheel::new(WHEEL_BUCKET_COUNT, WHEEL_RESOLUTION_MS),
            pending,
            send_batches,
            recv_packets: Vec::with_capacity(recv_batch_size),
            expired: Vec::with_capacity(concurrency),
            results: vec![None; domain_count],
            done: vec![false; domain_count],
            completed: 0,
            next_v4_socket: 0,
            next_v6_socket: 0,
        })
    }

    fn run(&mut self, domains: &[String]) -> Result<Vec<Option<ResolveResponse>>, AppError> {
        while self.completed < domains.len() {
            self.fill_send_queue(domains)?;
            self.flush_sends();
            self.poll_once()?;
            self.handle_timeouts();
        }

        Ok(self.results.clone())
    }

    fn fill_send_queue(&mut self, domains: &[String]) -> Result<(), AppError> {
        while self.slab.active_count() < self.config.concurrency {
            let Some(queued) = self.pending.pop_front() else {
                break;
            };

            if self.done[queued.domain_index as usize] {
                continue;
            }

            let resolver_index = rand::random::<usize>() % self.resolvers.len();
            let resolver = self.resolvers[resolver_index];
            let Some(socket_index) = self.pick_socket_for(resolver) else {
                return Err(AppError::new(format!(
                    "no DNS socket available for resolver '{resolver}'"
                )));
            };

            let slot = LookupSlot {
                domain_index: queued.domain_index,
                attempts: queued.attempt,
                resolver_index: resolver_index as u16,
            };
            let id = self.slab.insert(slot);

            let mut packet = OutPacket {
                addr: resolver,
                len: 0,
                buf: [0; super::socket::MAX_DNS_PACKET_SIZE],
            };

            let Some(len) = build_query(
                &mut packet.buf,
                id,
                &domains[queued.domain_index as usize],
                QTYPE_NS,
            ) else {
                let _ = self.slab.remove(id);
                self.finish_failure(queued.domain_index as usize);
                continue;
            };

            packet.len = len;
            self.wheel.insert(id, self.config.query_timeout_ms);
            self.send_batches[socket_index].push_back(packet);
        }

        Ok(())
    }

    fn flush_sends(&mut self) {
        for (index, batch) in self.send_batches.iter_mut().enumerate() {
            while !batch.is_empty() {
                let sent = {
                    let contiguous = batch.make_contiguous();
                    let limit = contiguous.len().min(self.config.send_batch_size);
                    send_batch(&self.sockets[index].socket, &contiguous[..limit])
                };

                if sent == 0 {
                    break;
                }

                for _ in 0..sent {
                    let _ = batch.pop_front();
                }
            }
        }
    }

    fn poll_once(&mut self) -> Result<(), AppError> {
        match self
            .poll
            .poll(&mut self.events, Some(Duration::from_millis(1)))
        {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => return Ok(()),
            Err(err) => return Err(AppError::io("failed while polling DNS sockets", err)),
        }

        let tokens = self
            .events
            .iter()
            .map(|event| event.token().0)
            .collect::<Vec<_>>();
        for token in tokens {
            self.drain_socket(token);
        }

        Ok(())
    }

    fn drain_socket(&mut self, socket_index: usize) {
        loop {
            let received = recv_batch(
                &self.sockets[socket_index].socket,
                self.config.recv_batch_size,
                &mut self.recv_packets,
            );

            if received == 0 {
                break;
            }

            while let Some(packet) = self.recv_packets.pop() {
                self.handle_packet(packet);
            }
        }
    }

    fn handle_packet(&mut self, packet: InPacket) {
        let Some(header) = parse_response_header(&packet.buf[..packet.len]) else {
            return;
        };

        let Some(slot) = self.slab.get(header.id) else {
            return;
        };

        if packet.addr != self.resolvers[slot.resolver_index as usize] {
            return;
        }

        let slot = self.slab.remove(header.id).expect("slot existed above");
        if is_definitive(header.rcode) {
            self.finish_success(
                slot.domain_index as usize,
                ResolveResponse {
                    rcode: header.rcode,
                    answer_count: header.answer_count,
                },
            );
        } else {
            self.retry_or_fail(slot);
        }
    }

    fn handle_timeouts(&mut self) {
        self.expired.clear();
        self.wheel.advance_into(&mut self.expired);

        let expired_ids = self.expired.drain(..).collect::<Vec<_>>();
        for id in expired_ids {
            if let Some(slot) = self.slab.remove(id) {
                self.retry_or_fail(slot);
            }
        }
    }

    fn retry_or_fail(&mut self, slot: LookupSlot) {
        if slot.attempts < self.config.max_attempts {
            self.pending.push_back(QueuedLookup {
                domain_index: slot.domain_index,
                attempt: slot.attempts + 1,
            });
        } else {
            self.finish_failure(slot.domain_index as usize);
        }
    }

    fn finish_success(&mut self, domain_index: usize, response: ResolveResponse) {
        if self.done[domain_index] {
            return;
        }

        self.done[domain_index] = true;
        self.results[domain_index] = Some(response);
        self.completed += 1;
    }

    fn finish_failure(&mut self, domain_index: usize) {
        if self.done[domain_index] {
            return;
        }

        self.done[domain_index] = true;
        self.results[domain_index] = None;
        self.completed += 1;
    }

    fn pick_socket_for(&mut self, resolver: std::net::SocketAddr) -> Option<usize> {
        let family = address_family(resolver);
        let start = match family {
            super::socket::AddressFamily::V4 => self.next_v4_socket,
            super::socket::AddressFamily::V6 => self.next_v6_socket,
        };

        for offset in 0..self.sockets.len() {
            let index = (start + offset) % self.sockets.len();
            if self.sockets[index].family == family {
                match family {
                    super::socket::AddressFamily::V4 => {
                        self.next_v4_socket = (index + 1) % self.sockets.len();
                    }
                    super::socket::AddressFamily::V6 => {
                        self.next_v6_socket = (index + 1) % self.sockets.len();
                    }
                }
                return Some(index);
            }
        }

        None
    }
}
