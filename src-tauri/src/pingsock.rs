//! Traceroute with Linux ping sockets (`SOCK_DGRAM` with `IPPROTO_ICMP`), the ones `ping`
//! uses. Unlike raw sockets they need neither root nor CAP_NET_RAW, only a group within
//! `net.ipv4.ping_group_range`, which most distributions open to every user. The routers'
//! time exceeded errors arrive on the socket's error queue (`IP_RECVERR`), as `tracepath`
//! reads them. It only does ICMP echo.

use std::io;
use std::mem::{size_of, size_of_val, zeroed};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use crate::HopSample;
use crate::echo::{GRACE, LOST, Outcome, reach, route};

/// Low bits of an echo's sequence number that hold the hop number (up to `MAX_TTL`). The
/// bits above count rounds, so that a late answer to an earlier round is not taken for one
/// to this round.
const TTL_BITS: u16 = 5;

/// What an answer read from the socket says: the echo's sequence number, who answered and
/// whether that was the target.
type Answer = (u16, IpAddr, bool);

/// Traces one target, a round at a time.
pub struct Pinger {
    socket: OwnedFd,
    target: IpAddr,
    /// The echo request. The kernel fills in its identifier and checksum; `send` writes
    /// the sequence number.
    request: Vec<u8>,
    timeout: Duration,
    /// Counts rounds, within the bits the sequence number leaves them.
    rounds: u16,
    /// The target's hop number in the last round, `None` if it did not answer.
    target_ttl: Option<u8>,
    /// The furthest hop number that answered in the last round, 0 if none did.
    answered: u8,
    /// Rounds in a row in which the target did not answer.
    unknown_rounds: u32,
}

impl Pinger {
    /// `packet_size` is the size of a probe's whole IP packet, `timeout` how long a probe
    /// may go unanswered before it counts as lost.
    pub fn new(target: IpAddr, packet_size: u16, timeout: Duration) -> Result<Self, String> {
        let (level, recverr, echo, ip_header) = match target {
            IpAddr::V4(_) => (libc::IPPROTO_IP, libc::IP_RECVERR, 8, 20),
            IpAddr::V6(_) => (libc::IPPROTO_IPV6, libc::IPV6_RECVERR, 128, 40),
        };
        let socket = open(target).map_err(|e| {
            format!("cannot open a ping socket: {e} (the user's group must be within net.ipv4.ping_group_range)")
        })?;
        set_option(&socket, level, recverr, 1).map_err(|e| format!("cannot ask for ICMP errors: {e}"))?;
        // The ICMP part of the packet: an 8-byte echo header and the payload.
        let mut request = vec![0u8; usize::from(packet_size).saturating_sub(ip_header).max(8)];
        request[0] = echo;
        Ok(Self {
            socket,
            target,
            request,
            timeout,
            rounds: 0,
            target_ttl: None,
            answered: 0,
            unknown_rounds: 0,
        })
    }

    /// Sends the echo for one hop number.
    fn send(&mut self, ttl: u8, sequence: u16) -> io::Result<()> {
        let (level, name) = match self.target {
            IpAddr::V4(_) => (libc::IPPROTO_IP, libc::IP_TTL),
            IpAddr::V6(_) => (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS),
        };
        set_option(&self.socket, level, name, i32::from(ttl))?;
        self.request[6..8].copy_from_slice(&sequence.to_be_bytes());
        let (address, length) = socket_address(self.target);
        // SAFETY: the request and the address are valid for the lengths passed.
        let sent = unsafe {
            libc::sendto(
                self.socket.as_raw_fd(),
                self.request.as_ptr().cast(),
                self.request.len(),
                0,
                (&raw const address).cast(),
                length,
            )
        };
        if sent < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// An echo reply from the target. `None` when none is waiting; `Some(None)` for
    /// something else that was read.
    fn read_reply(&self) -> Option<Option<Answer>> {
        let mut data = [0u8; 2048];
        // SAFETY: all zeroes is a valid socket address.
        let mut from: libc::sockaddr_storage = unsafe { zeroed() };
        let mut length = size_of_val(&from) as libc::socklen_t;
        // SAFETY: the buffers are valid for the lengths passed.
        let read = unsafe {
            libc::recvfrom(
                self.socket.as_raw_fd(),
                data.as_mut_ptr().cast(),
                data.len(),
                0,
                (&raw mut from).cast(),
                &mut length,
            )
        };
        // Nothing waiting, or an error the socket reports once, such as that of an ICMP error
        // now on the error queue. Either way reading stops until the next wait says there is more.
        if read < 0 {
            return None;
        }
        let reply = if self.target.is_ipv4() { 0 } else { 129 };
        // SAFETY: the kernel wrote a socket address of the socket's family into `from`.
        let addr = unsafe { read_address((&raw const from).cast()) };
        Some(
            addr.filter(|_| read >= 8 && data[0] == reply)
                .map(|addr| (u16::from_be_bytes([data[6], data[7]]), addr, true)),
        )
    }

    /// An ICMP error from the error queue: time exceeded from a router, or another error
    /// from a router or the target. `None` when the queue is empty.
    fn read_error(&self) -> Option<Option<Answer>> {
        // The echo the error is about, from its header on.
        let mut data = [0u8; 2048];
        // u64 keeps the control messages aligned.
        let mut control = [0u64; 64];
        let mut iov = libc::iovec {
            iov_base: data.as_mut_ptr().cast(),
            iov_len: data.len(),
        };
        // SAFETY: all zeroes is a valid message header; the buffers it points to live until
        // the messages are read.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &raw mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = size_of_val(&control);
        // SAFETY: the message header and its buffers are valid.
        let read = unsafe { libc::recvmsg(self.socket.as_raw_fd(), &raw mut message, libc::MSG_ERRQUEUE) };
        if read < 0 {
            return None;
        }
        let mut offender = None;
        // SAFETY: the kernel wrote `msg_controllen` bytes of control messages. An IP_RECVERR
        // message holds a `sock_extended_err` followed by the address of who sent the error.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                let kind = ((*header).cmsg_level, (*header).cmsg_type);
                if kind == (libc::SOL_IP, libc::IP_RECVERR) || kind == (libc::SOL_IPV6, libc::IPV6_RECVERR) {
                    let error = libc::CMSG_DATA(header).cast::<libc::sock_extended_err>();
                    let origin = error.read_unaligned().ee_origin;
                    if origin == libc::SO_EE_ORIGIN_ICMP || origin == libc::SO_EE_ORIGIN_ICMP6 {
                        offender = read_address(error.add(1).cast());
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        Some(
            offender
                .filter(|_| read >= 8)
                .map(|addr| (u16::from_be_bytes([data[6], data[7]]), addr, addr == self.target)),
        )
    }

    /// Takes in every answer that has arrived for this round's probes.
    fn collect(&self, sent: &[Instant], probes: &mut [Option<(Outcome, Instant)>]) {
        while let Some(answer) = self.read_error().or_else(|| self.read_reply()) {
            let Some((sequence, addr, reached)) = answer else { continue };
            let ttl = usize::from(sequence & ((1 << TTL_BITS) - 1));
            if sequence >> TTL_BITS != self.rounds || ttl == 0 || ttl > probes.len() || probes[ttl - 1].is_some() {
                continue;
            }
            let now = Instant::now();
            let rtt = now.duration_since(sent[ttl - 1]).as_secs_f64() * 1000.0;
            let outcome = Outcome {
                addr: Some(addr),
                rtt: Some((rtt * 100.0).round() / 100.0),
                reached,
            };
            probes[ttl - 1] = Some((outcome, now));
        }
    }

    /// Waits until the socket has something to read, or `time` has passed.
    fn wait(&self, time: Duration) {
        let mut poll = libc::pollfd {
            fd: self.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Rounded up, so that the wait does not end just before its time.
        let ms = time.as_millis().min(i32::MAX as u128 - 1) as i32 + 1;
        // SAFETY: one valid pollfd. Errors on the queue wake it as POLLERR.
        unsafe { libc::poll(&mut poll, 1, ms) };
    }

    /// Probes every hop once and returns the route's hops. Takes at least `interval`; longer
    /// only while the target has not answered, and then no longer than the reply timeout.
    pub fn round(&mut self, interval: Duration) -> Vec<HopSample> {
        let start = Instant::now();
        self.rounds = (self.rounds + 1) & (u16::MAX >> TTL_BITS);
        // If the route got longer the target does not answer at its old hop number, and
        // the next round probes all the way to find it again.
        let limit = usize::from(reach(self.target_ttl, self.answered, self.unknown_rounds));
        let mut sent = vec![start; limit];
        let mut probes = vec![None; limit];
        for ttl in 1..=limit {
            sent[ttl - 1] = Instant::now();
            if self.send(ttl as u8, self.rounds << TTL_BITS | ttl as u16).is_err() {
                probes[ttl - 1] = Some((LOST, Instant::now()));
            }
            // Answers that are already in are taken now. They are timed when they are
            // read, so a near hop would otherwise be charged the time it takes to send the
            // remaining probes.
            self.collect(&sent, &mut probes);
        }
        loop {
            self.collect(&sent, &mut probes);
            let reached = probes
                .iter()
                .flatten()
                .filter(|(outcome, _)| outcome.reached)
                .map(|(_, at)| *at)
                .min();
            let end = if probes.iter().all(Option::is_some) {
                start + interval
            } else {
                reached.map_or(start + self.timeout, |at| at + GRACE).max(start + interval)
            };
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            self.wait(left);
        }
        let probes: Vec<Outcome> = probes.into_iter().map(|probe| probe.map_or(LOST, |(outcome, _)| outcome)).collect();
        let (hops, target_ttl) = route(&probes);
        self.target_ttl = target_ttl;
        self.answered = hops.iter().rev().find(|hop| hop.rtt.is_some()).map_or(0, |hop| hop.ttl);
        self.unknown_rounds = if target_ttl.is_some() { 0 } else { self.unknown_rounds + 1 };
        hops
    }
}

/// Opens a ping socket for the target's family.
fn open(target: IpAddr) -> io::Result<OwnedFd> {
    let (domain, protocol) = match target {
        IpAddr::V4(_) => (libc::AF_INET, libc::IPPROTO_ICMP),
        IpAddr::V6(_) => (libc::AF_INET6, libc::IPPROTO_ICMPV6),
    };
    let kind = libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    // SAFETY: a plain call; a descriptor it returns is owned from here on.
    let fd = unsafe { libc::socket(domain, kind, protocol) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether this user may open ping sockets for the target's family.
pub fn available(target: IpAddr) -> bool {
    open(target).is_ok()
}

/// Sets an integer socket option.
fn set_option(socket: &OwnedFd, level: i32, name: i32, value: i32) -> io::Result<()> {
    // SAFETY: the value is an int that lives through the call, and its size is passed.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (&raw const value).cast(),
            size_of::<i32>() as libc::socklen_t,
        )
    };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// An address as the kernel takes it, with its length.
fn socket_address(addr: IpAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: all zeroes is a valid socket address, and the storage is large and aligned
    // enough for either family.
    let mut storage: libc::sockaddr_storage = unsafe { zeroed() };
    let length = match addr {
        IpAddr::V4(v4) => {
            let address = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: 0,
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.octets()),
                },
                sin_zero: [0; 8],
            };
            unsafe { (&raw mut storage).cast::<libc::sockaddr_in>().write(address) };
            size_of::<libc::sockaddr_in>()
        }
        IpAddr::V6(v6) => {
            // SAFETY: as above.
            let mut address: libc::sockaddr_in6 = unsafe { zeroed() };
            address.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            address.sin6_addr.s6_addr = v6.octets();
            unsafe { (&raw mut storage).cast::<libc::sockaddr_in6>().write(address) };
            size_of::<libc::sockaddr_in6>()
        }
    };
    (storage, length as libc::socklen_t)
}

/// The IP address in a socket address the kernel wrote, `None` for another family.
///
/// # Safety
/// `address` points to a socket address as large as its family says.
unsafe fn read_address(address: *const libc::sockaddr) -> Option<IpAddr> {
    // SAFETY: as the caller promises; the address may sit unaligned in a control message.
    unsafe {
        match i32::from((&raw const (*address).sa_family).read_unaligned()) {
            libc::AF_INET => {
                let address = address.cast::<libc::sockaddr_in>().read_unaligned();
                Some(IpAddr::V4(Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes())))
            }
            libc::AF_INET6 => {
                let address = address.cast::<libc::sockaddr_in6>().read_unaligned();
                Some(IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live check, needs network but not root:
    /// `cargo test --lib pingsock -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_trace_reaches_the_target() {
        let target: IpAddr = std::env::var("TRACEPLOTTER_TARGET")
            .unwrap_or("1.1.1.1".into())
            .parse()
            .unwrap();
        let mut pinger = Pinger::new(target, 84, Duration::from_secs(1)).unwrap();
        let mut reached = 0;
        for _ in 0..3 {
            let hops = pinger.round(Duration::from_millis(300));
            println!("{hops:?}");
            reached += usize::from(hops.last().is_some_and(|hop| hop.addr == Some(target)));
        }
        assert!(reached > 0, "the target never answered");
    }
}
