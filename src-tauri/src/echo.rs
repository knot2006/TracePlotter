//! What traces by ICMP echo share, whichever system API sends the echoes: the Windows
//! ICMP API (winicmp.rs) or Linux ping sockets (pingsock.rs). A round probes every hop
//! number at once and keeps the answers up to the target.

use std::net::IpAddr;
use std::time::Duration;

use crate::HopSample;

/// Furthest hop probed.
pub const MAX_TTL: u8 = 30;
/// While the target's hop number is not known, how many hops past the last one that
/// answered a round probes, and how often a round probes all the way instead.
pub const GAP: u8 = 3;
pub const SWEEP_ROUNDS: u32 = 10;
/// After the target answered, how long a round still waits for slower hops.
pub const GRACE: Duration = Duration::from_millis(100);

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

pub const LOST: Outcome = Outcome {
    addr: None,
    rtt: None,
    reached: false,
};

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
pub fn reach(target: Option<u8>, answered: u8, unknown_rounds: u32) -> u8 {
    match target {
        Some(ttl) => ttl,
        None if unknown_rounds.is_multiple_of(SWEEP_ROUNDS) => MAX_TTL,
        None => answered.saturating_add(GAP).min(MAX_TTL),
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
}
