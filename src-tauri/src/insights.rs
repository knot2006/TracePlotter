use super::*;

/// Detailed statistics are read on request, rather than adding work to every graph refresh.
#[derive(Default, Serialize)]
pub(super) struct Detail {
    ttl: u8,
    addr: Option<IpAddr>,
    total: Tally,
    median: Option<f64>,
    p95: Option<f64>,
    p99: Option<f64>,
    longest_lost: u64,
    longest_loss_seconds: f64,
    #[serde(skip)]
    replies: BTreeMap<u64, u64>,
    #[serde(skip)]
    previous: Option<f64>,
    #[serde(skip)]
    last_time: Option<u64>,
    #[serde(skip)]
    lost: u64,
    #[serde(skip)]
    lost_start: u64,
}

impl Detail {
    fn add(&mut self, t: u64, rtt: Option<f64>, interval: u64) {
        // A pause or an absent hop must not join two loss bursts into one outage.
        if self
            .last_time
            .is_some_and(|last| t.saturating_sub(last) > (3 * interval).max(5000))
        {
            self.lost = 0;
            self.previous = None;
        }
        self.total.add(rtt, self.previous);
        self.last_time = Some(t);
        if let Some(rtt) = rtt {
            self.previous = Some(rtt);
            // A tenth of a millisecond keeps memory bounded by latency variation, not days.
            *self
                .replies
                .entry((rtt.max(0.0) * 10.0).round() as u64)
                .or_default() += 1;
            self.lost = 0;
        } else {
            if self.lost == 0 {
                self.lost_start = t;
            }
            self.lost += 1;
            self.longest_lost = self.longest_lost.max(self.lost);
            self.longest_loss_seconds = self
                .longest_loss_seconds
                .max((t - self.lost_start + interval) as f64 / 1000.0);
        }
    }

    fn finish(&mut self) {
        let at_rank = |rank: u64| {
            let mut count = 0;
            self.replies.iter().find_map(|(rtt, n)| {
                count += n;
                (count >= rank).then_some(*rtt as f64 / 10.0)
            })
        };
        let count = u64::from(self.total.received);
        self.median = at_rank(count.div_ceil(2)).map(|lower| {
            if count.is_multiple_of(2) {
                (lower + at_rank(count / 2 + 1).unwrap_or(lower)) / 2.0
            } else {
                lower
            }
        });
        self.p95 = at_rank((count as f64 * 0.95).ceil() as u64);
        self.p99 = at_rank((count as f64 * 0.99).ceil() as u64);
    }
}

fn add_round(details: &mut BTreeMap<u8, Detail>, sample: Sample, interval: u64) {
    for detail in details.values_mut() {
        if detail.ttl != 0 && !sample.hops.iter().any(|hop| hop.ttl == detail.ttl) {
            detail.lost = 0;
            detail.previous = None;
        }
    }
    let reply = sample.hops.last().and_then(|hop| hop.rtt);
    for (ttl, addr, rtt) in sample
        .hops
        .into_iter()
        .map(|hop| (hop.ttl, hop.addr, hop.rtt))
        .chain([(0, None, reply)])
    {
        let detail = details.entry(ttl).or_insert_with(|| Detail {
            ttl,
            ..Default::default()
        });
        detail.addr = addr.or(detail.addr);
        detail.add(sample.t, rtt, interval);
    }
}

/// Exact period boundaries and raw rounds, including pauses, for reports and the statistics dialog.
#[tauri::command(async)]
pub(super) fn history_details(
    session: String,
    from: u64,
    to: u64,
    interval: u64,
) -> Result<Vec<Detail>, String> {
    let dir = history_dir(&session)?;
    let mut details = BTreeMap::new();
    for (_, path) in day_files(&dir)
        .into_iter()
        .filter(|(day, _)| (from / DAY_MS..=to / DAY_MS).contains(day))
    {
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if !line_time(&line).is_some_and(|t| (from..to).contains(&t)) {
                continue;
            }
            if let Ok(sample) = serde_json::from_str::<Sample>(&line) {
                add_round(&mut details, sample, interval.clamp(100, 60_000));
            }
        }
    }
    for detail in details.values_mut() {
        detail.finish();
    }
    Ok(details.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_losses_and_pauses() {
        let mut detail = Detail::default();
        for (t, rtt) in [
            (0, Some(10.04)),
            (1000, None),
            (2000, None),
            (3000, Some(20.0)),
            (4000, None),
            (20_000, None),
        ] {
            detail.add(t, rtt, 1000);
        }
        detail.finish();
        assert_eq!(detail.total.sent, 6);
        assert_eq!(detail.median, Some(15.0));
        assert_eq!(detail.p95, Some(20.0));
        assert_eq!(detail.p99, Some(20.0));
        assert_eq!(detail.longest_lost, 2);
        assert_eq!(detail.longest_loss_seconds, 2.0);
        let mut empty = Detail::default();
        empty.finish();
        assert_eq!(empty.median, None);
    }

    #[test]
    fn round_trip_survives_route_length_changes() {
        let mut details = BTreeMap::new();
        for (t, ttl, rtt) in [
            (1000, 3, Some(12.0)),
            (2000, 4, None),
            (3000, 2, Some(14.0)),
        ] {
            add_round(
                &mut details,
                Sample {
                    t,
                    hops: vec![HopSample {
                        ttl,
                        addr: None,
                        rtt,
                    }],
                },
                1000,
            );
        }
        assert_eq!(details[&0].total.sent, 3);
        assert_eq!(details[&0].total.received, 2);
        assert_eq!(details[&0].longest_lost, 1);
    }

    #[test]
    fn absent_hops_do_not_join_loss_bursts() {
        let mut details = BTreeMap::new();
        for (t, ttl) in [(1000, 3), (2000, 2), (3000, 3)] {
            add_round(
                &mut details,
                Sample {
                    t,
                    hops: vec![HopSample {
                        ttl,
                        addr: None,
                        rtt: None,
                    }],
                },
                1000,
            );
        }
        assert_eq!(details[&3].longest_lost, 1);
        assert_eq!(details[&0].longest_lost, 3);
    }
}
