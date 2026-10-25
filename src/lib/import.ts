import { INTERVALS } from "./settings.ts";
import { dateOnly, timeOfDay } from "./time.ts";
import type { Sample } from "./trace.ts";

/** The first line of a samples export (see `exportCsv` in TraceTab.svelte). */
const HEADER = "time,hop,ip,rtt_ms";

/**
 * The rounds of a samples export, oldest first, as the history file holds them. The
 * export has one line per probe; the probes of a round share its time. Lines that cannot
 * be read are skipped. Throws when the text is not a samples export or holds no probe.
 */
export function parseSamplesCsv(text: string): Sample[] {
  const lines = text.split(/\r?\n/);
  // Spreadsheets may have put a byte order mark in front.
  if (lines[0].replace(/^﻿/, "").trim() !== HEADER) {
    throw new Error(`not a TracePlotter samples export: its first line is not ${HEADER}`);
  }
  const rounds = new Map<number, Sample>();
  for (let i = 1; i < lines.length; i++) {
    const [time, hop, ip, rtt] = lines[i].split(",");
    const t = Date.parse(time);
    const ttl = Number(hop);
    if (!Number.isFinite(t) || !Number.isInteger(ttl) || ttl < 1 || ttl > 255) continue;
    const ms = rtt ? Number(rtt) : NaN;
    let round = rounds.get(t);
    if (!round) rounds.set(t, (round = { t, hops: [] }));
    round.hops.push({
      ttl,
      // Only what an address is made of: the backend drops a round with anything else.
      addr: ip && /^[0-9a-fA-F:.]+$/.test(ip) ? ip : null,
      rtt: Number.isFinite(ms) && ms >= 0 ? ms : null,
    });
  }
  if (!rounds.size) throw new Error("the export holds no samples");
  const samples = [...rounds.values()].sort((a, b) => a.t - b.t);
  for (const sample of samples) sample.hops.sort((a, b) => a.ttl - b.ttl);
  return samples;
}

/** The interval the rounds were probed at: the offered one nearest to their usual distance. */
export function guessInterval(samples: Sample[]): number {
  const gaps = samples.slice(1).map((sample, i) => sample.t - samples[i].t).sort((a, b) => a - b);
  const usual = gaps[Math.floor(gaps.length / 2)] ?? 1000;
  return INTERVALS.reduce((best, ms) => (Math.abs(ms - usual) < Math.abs(best - usual) ? ms : best));
}

/**
 * A date or time as the host writes it, made fit for a file name: a dot between its parts
 * stays, anything else between them (slashes, colons, spaces) becomes a dash.
 */
const fileSafe = (text: string) =>
  text.replace(/[^\p{L}\p{N}]+/gu, (between) => (between === "." ? "." : "-")).replace(/^[-.]+|[-.]+$/g, "");

/**
 * The name an export is saved under: the host, what the file holds, and the date and
 * time it was made at, in the host's own format. With Polish regional settings, for one,
 * "TracePlotter_example.com_samples_14.10.2026_04-19-18.csv".
 */
export function exportName(
  host: string,
  kind: "report" | "samples" | "graph" | "stretches" | "incident",
  extension: string,
  at = new Date(),
) {
  const t = at.getTime() / 1000;
  return `TracePlotter_${host}_${kind}_${fileSafe(dateOnly(t))}_${fileSafe(timeOfDay(t))}.${extension}`;
}

// Names of samples exports: the one `exportName` gives, whatever the date format, and the
// one of earlier versions.
const SAMPLES_NAMES = [/^TracePlotter_(.+)_samples_.+\.csv$/i, /^traceplotter-(.+)-\d{8}-\d{6}\.csv$/i];

/** The host an export was made for, from the name the app gave the file, else the file's name without ".csv". */
export function hostFromFileName(name: string) {
  for (const pattern of SAMPLES_NAMES) {
    const host = pattern.exec(name)?.[1];
    if (host) return host;
  }
  return name.replace(/\.csv$/i, "");
}
