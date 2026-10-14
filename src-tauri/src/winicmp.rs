//! Traceroute with the Windows ICMP API (`IcmpSendEcho2`), the one `ping` and `tracert`
//! use. Unlike raw sockets it needs neither Administrator rights nor a firewall rule.
//! It only does ICMP echo.

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    ICMP_ECHO_REPLY, ICMPV6_ECHO_REPLY_LH, IP_OPTION_INFORMATION, IP_REQ_TIMED_OUT, IP_SUCCESS,
    Icmp6CreateFile, Icmp6ParseReplies, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile,
    IcmpParseReplies, IcmpSendEcho2,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET6, SOCKADDR_IN6};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::Threading::{INFINITE, SleepEx};

use crate::HopSample;

/// Furthest hop probed.
const MAX_TTL: u8 = 30;
/// While the target's hop number is not known, how many hops past the last one that
/// answered a round probes, and how often a round probes all the way instead.
const GAP: u8 = 3;
const SWEEP_ROUNDS: u32 = 10;
/// After the target answered, how long a round still waits for slower hops.
const GRACE: Duration = Duration::from_millis(100);
/// Room in a reply buffer beside the echoed payload: the reply header, an ICMP error
/// message and the I/O status block the API asks for.
const REPLY_OVERHEAD: usize = 256;

/// How one probe ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outcome {
    /// Who answered, `None` when nobody did in time.
    pub addr: Option<IpAddr>,
    /// Round trip in ms.
    pub rtt: Option<f64>,
    /// The answer came from the target itself, not from a router on the way.
    pub reached: bool,
}

const LOST: Outcome = Outcome {
    addr: None,
    rtt: None,
    reached: false,
};

/// One probe per hop number. The API writes into `reply` and then calls `completed` with
/// a pointer to the slot, so slots never move and are not freed while `pending`.
struct Slot {
    target: IpAddr,
    reply: UnsafeCell<Box<[u8]>>,
    sent: Cell<Instant>,
    /// Sent and not completed yet. The API completes every probe: by a reply or by its timeout.
    pending: Cell<bool>,
    /// The result of the last completed probe and when it completed; taken by `round`.
    outcome: Cell<Option<(Outcome, Instant)>>,
}

impl Slot {
    /// Reads the reply the API left in the buffer.
    fn finish(&self) {
        let now = Instant::now();
        // SAFETY: the probe has completed, so the API no longer writes to the buffer, and
        // nothing else borrows it. The buffer is larger than either reply struct.
        let (addr, status, api_rtt) = unsafe {
            let buffer = &mut *self.reply.get();
            let (pointer, size) = (buffer.as_mut_ptr().cast::<c_void>(), buffer.len() as u32);
            if self.target.is_ipv4() {
                IcmpParseReplies(pointer, size);
                let reply = pointer.cast::<ICMP_ECHO_REPLY>().read_unaligned();
                let addr = Ipv4Addr::from(reply.Address.to_ne_bytes());
                (IpAddr::V4(addr), reply.Status, reply.RoundTripTime)
            } else {
                Icmp6ParseReplies(pointer, size);
                let reply = pointer.cast::<ICMPV6_ECHO_REPLY_LH>().read_unaligned();
                let words = reply.Address.sin6_addr;
                // The words are in network byte order, as they sit in memory.
                let addr = Ipv6Addr::from(words.map(u16::from_be));
                (IpAddr::V6(addr), reply.Status, reply.RoundTripTime)
            }
        };
        // A probe that timed out leaves no address; one that failed some other way may.
        let outcome = if addr.is_unspecified() || status == IP_REQ_TIMED_OUT {
            LOST
        } else {
            // The API counts whole milliseconds, so the time is taken here instead. That
            // includes the wait for this thread to run; the API's count caps it.
            let measured = now.duration_since(self.sent.get()).as_secs_f64() * 1000.0;
            let rtt = measured.min(f64::from(api_rtt) + 1.0);
            Outcome {
                addr: Some(addr),
                rtt: Some((rtt * 100.0).round() / 100.0),
                reached: status == IP_SUCCESS || addr == self.target,
            }
        };
        self.outcome.set(Some((outcome, now)));
        self.pending.set(false);
    }
}

/// Called by Windows on the probing thread, while it sleeps alertably, when a probe completes.
unsafe extern "system" fn completed(slot: *mut c_void, _: *mut IO_STATUS_BLOCK, _: u32) {
    // SAFETY: `slot` is the pointer `send` passed, to a slot that outlives its probes.
    unsafe { (*slot.cast::<Slot>()).finish() }
}

/// Cuts a round's probes, in hop order, down to the route: it ends at the first hop where
/// the target answered. With a silent target it ends one hop past the last answer, as the
/// raw-socket tracer's rounds do. Returns the hops and the target's hop number.
pub fn route(probes: &[Outcome]) -> (Vec<HopSample>, Option<u8>) {
    let target = probes.iter().position(|probe| probe.reached);
    let answered = probes.iter().rposition(|probe| probe.rtt.is_some());
    let end = target.or(answered.map(|last| last + 1)).unwrap_or(0);
    let hops = probes
        .iter()
        .take(end + 1)
        .enumerate()
        .map(|(i, probe)| HopSample {
            ttl: i as u8 + 1,
            addr: probe.addr,
            rtt: probe.rtt,
        })
        .collect();
    (hops, target.map(|i| i as u8 + 1))
}

/// The furthest hop number a round probes. With a known target that is the target's. While
/// it is unknown, every `SWEEP_ROUNDS`th round, starting with the first, probes all the
/// way to find it; the rounds between stop `GAP` hops past the last hop that answered.
/// A target that never answers would otherwise draw `MAX_TTL` probes at once every round,
/// and routers that limit their ICMP replies then stop answering for the nearer hops.
fn reach(target: Option<u8>, answered: u8, unknown_rounds: u32) -> u8 {
    match target {
        Some(ttl) => ttl,
        None if unknown_rounds.is_multiple_of(SWEEP_ROUNDS) => MAX_TTL,
        None => answered.saturating_add(GAP).min(MAX_TTL),
    }
}

/// Traces one target, a round at a time. Must stay on the thread that created it: probes
/// complete on that thread only.
pub struct Pinger {
    handle: HANDLE,
    target: IpAddr,
    payload: Vec<u8>,
    timeout_ms: u32,
    slots: Box<[Slot]>,
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
        // IP header plus the 8 bytes of an ICMP echo header.
        let headers = if target.is_ipv4() { 28 } else { 48 };
        let payload = vec![0u8; usize::from(packet_size).saturating_sub(headers)];
        // SAFETY: plain calls without arguments.
        let handle = unsafe {
            if target.is_ipv4() {
                IcmpCreateFile()
            } else {
                Icmp6CreateFile()
            }
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "cannot open the Windows ICMP API: {}",
                std::io::Error::last_os_error()
            ));
        }
        let slots = (0..MAX_TTL)
            .map(|_| Slot {
                target,
                reply: UnsafeCell::new(vec![0u8; payload.len() + REPLY_OVERHEAD].into()),
                sent: Cell::new(Instant::now()),
                pending: Cell::new(false),
                outcome: Cell::new(None),
            })
            .collect();
        Ok(Self {
            handle,
            target,
            payload,
            timeout_ms: timeout.as_millis().min(60_000) as u32,
            slots,
            target_ttl: None,
            answered: 0,
            unknown_rounds: 0,
        })
    }

    /// Sends the probe for one hop number. A probe that cannot be sent is lost at once.
    fn send(&self, ttl: u8) {
        let slot = &self.slots[usize::from(ttl) - 1];
        let options = IP_OPTION_INFORMATION {
            Ttl: ttl,
            ..Default::default()
        };
        slot.outcome.set(None);
        slot.pending.set(true);
        slot.sent.set(Instant::now());
        // SAFETY: the payload and options are read during the call. The reply buffer and
        // the slot are written later, on completion; both live until `drop`, which waits
        // for every pending probe.
        let pending = unsafe {
            let buffer = &mut *slot.reply.get();
            buffer.fill(0);
            let context = std::ptr::from_ref(slot).cast::<c_void>();
            let (data, size) = (self.payload.as_ptr().cast(), self.payload.len() as u16);
            let (reply, reply_size) = (buffer.as_mut_ptr().cast(), buffer.len() as u32);
            let sent = match self.target {
                IpAddr::V4(target) => IcmpSendEcho2(
                    self.handle,
                    std::ptr::null_mut(),
                    Some(completed),
                    context,
                    u32::from_ne_bytes(target.octets()),
                    data,
                    size,
                    &options,
                    reply,
                    reply_size,
                    self.timeout_ms,
                ),
                IpAddr::V6(target) => {
                    let mut to: SOCKADDR_IN6 = std::mem::zeroed();
                    to.sin6_family = AF_INET6;
                    to.sin6_addr.u.Byte = target.octets();
                    // From the unspecified address: Windows picks the source.
                    let mut from: SOCKADDR_IN6 = std::mem::zeroed();
                    from.sin6_family = AF_INET6;
                    Icmp6SendEcho2(
                        self.handle,
                        std::ptr::null_mut(),
                        Some(completed),
                        context,
                        &from,
                        &to,
                        data,
                        size,
                        &options,
                        reply,
                        reply_size,
                        self.timeout_ms,
                    )
                }
            };
            // With a completion routine the call does not wait: it returns 0 with
            // ERROR_IO_PENDING when the probe is out, and anything else is a failure
            // without a completion to come.
            sent == 0 && GetLastError() == ERROR_IO_PENDING
        };
        if !pending {
            slot.outcome.set(Some((LOST, Instant::now())));
            slot.pending.set(false);
        }
    }

    /// Probes every hop once and returns the route's hops. Takes at least `interval`; longer
    /// only while the target has not answered, and then no longer than the reply timeout.
    pub fn round(&mut self, interval: Duration) -> Vec<HopSample> {
        let start = Instant::now();
        // If the route got longer the target does not answer at its old hop number, and
        // the next round probes all the way to find it again.
        let limit = usize::from(reach(self.target_ttl, self.answered, self.unknown_rounds));
        for ttl in 1..=limit {
            // A probe still out from an earlier round keeps its slot until it completes.
            if !self.slots[ttl - 1].pending.get() {
                self.send(ttl as u8);
            }
            // Lets replies that are already in complete now. They are timed when their
            // completion runs, so a near hop would otherwise be charged the time it
            // takes to send the remaining probes.
            // SAFETY: an alertable sleep of no length; it only runs queued completions.
            unsafe { SleepEx(0, 1) };
        }
        let slots = &self.slots[..limit];
        loop {
            let reached = slots
                .iter()
                .filter_map(|slot| slot.outcome.get())
                .filter(|(outcome, _)| outcome.reached)
                .map(|(_, at)| at)
                .min();
            let end = if slots.iter().all(|slot| !slot.pending.get()) {
                Some(start + interval)
            } else {
                reached.map(|at| (start + interval).max(at + GRACE))
            };
            let wait = match end.map(|end| end.saturating_duration_since(Instant::now())) {
                Some(left) if left.is_zero() => break,
                Some(left) => left.as_millis() as u32 + 1,
                None => INFINITE,
            };
            // SAFETY: an alertable sleep; it returns early after probes completed.
            unsafe { SleepEx(wait, 1) };
        }
        let probes: Vec<Outcome> = slots
            .iter()
            .map(|slot| slot.outcome.take().map_or(LOST, |(outcome, _)| outcome))
            .collect();
        let (hops, target_ttl) = route(&probes);
        self.target_ttl = target_ttl;
        self.answered = hops.iter().rev().find(|hop| hop.rtt.is_some()).map_or(0, |hop| hop.ttl);
        self.unknown_rounds = if target_ttl.is_some() { 0 } else { self.unknown_rounds + 1 };
        hops
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        // Probes still out write to their slots when they complete, so wait for them.
        while self.slots.iter().any(|slot| slot.pending.get()) {
            // SAFETY: an alertable sleep, see `round`.
            unsafe { SleepEx(INFINITE, 1) };
        }
        // SAFETY: the handle came from `IcmpCreateFile` and is closed once.
        unsafe { IcmpCloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(addr: &str, reached: bool) -> Outcome {
        Outcome {
            addr: Some(addr.parse().unwrap()),
            rtt: Some(1.0),
            reached,
        }
    }

    #[test]
    fn route_ends_at_the_target_or_one_past_the_last_answer() {
        let hop = answer("10.0.0.1", false);
        let target = answer("10.0.0.9", true);
        let ttls = |probes: &[Outcome]| {
            let (hops, target) = route(probes);
            (hops.iter().map(|hop| hop.ttl).collect::<Vec<_>>(), target)
        };
        // Probes past the target also reach it; the route stops at the first.
        assert_eq!(ttls(&[hop, LOST, target, target]), (vec![1, 2, 3], Some(3)));
        assert_eq!(ttls(&[hop, hop, LOST, LOST, LOST]), (vec![1, 2, 3], None));
        assert_eq!(ttls(&[hop, hop]), (vec![1, 2], None));
        assert_eq!(ttls(&[LOST, LOST, LOST]), (vec![1], None));
        assert_eq!(route(&[hop, LOST]).0[1].rtt, None);
    }

    #[test]
    fn reach_probes_all_the_way_only_now_and_then_for_a_silent_target() {
        assert_eq!(reach(Some(9), 9, 0), 9);
        // The first round, and the round after the target stopped answering.
        assert_eq!(reach(None, 0, 0), MAX_TTL);
        assert_eq!(reach(None, 6, 1), 6 + GAP);
        assert_eq!(reach(None, 6, SWEEP_ROUNDS), MAX_TTL);
        assert_eq!(reach(None, MAX_TTL, 3), MAX_TTL);
        assert_eq!(reach(None, 0, 1), GAP);
    }

    /// Live check, needs network but not Administrator:
    /// `cargo test --lib winicmp -- --ignored --nocapture`
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
