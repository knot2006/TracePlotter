/** What the summary shows, with the field names the registries use for it. The first one found wins. */
const FACTS: { label: string; keys: string[] }[] = [
  { label: "Organisation", keys: ["org-name", "orgname", "owner", "organization", "descr"] },
  { label: "Network", keys: ["netname"] },
  { label: "Range", keys: ["inetnum", "inet6num", "netrange"] },
  { label: "Route", keys: ["route", "route6", "cidr"] },
  { label: "AS number", keys: ["origin", "originas", "aut-num"] },
  { label: "Country", keys: ["country"] },
  { label: "Abuse contact", keys: ["abuse", "orgabuseemail", "abuse-mailbox"] },
];

/**
 * The few facts people look up an address for, picked out of a whois answer. Registries
 * name the same thing differently (RIPE `inetnum`, ARIN `NetRange`), so each fact takes
 * the first of several fields. Facts the answer does not hold are left out.
 */
export function whoisFacts(text: string): { label: string; value: string }[] {
  const fields = new Map<string, string>();
  const keep = (key: string, value: string) => !fields.has(key) && fields.set(key, value);
  const lines = text.split(/\r?\n/);
  const field = (line: string) => /^([A-Za-z0-9-]+):\s*(\S.*)$/.exec(line);
  // An answer can list several networks, the widest first: a provider's block, then the
  // customer's part of it. The facts are those of the last, most specific one.
  const ranges = FACTS.find((fact) => fact.label === "Range")!.keys;
  const last = lines.findLastIndex((line) => ranges.includes(field(line)?.[1].toLowerCase() ?? ""));
  lines.forEach((line, i) => {
    // RIPE gives the abuse address only in a comment line, above the network.
    const abuse = /^% Abuse contact for .* is '(.+)'/.exec(line);
    if (abuse) keep("abuse", abuse[1]);
    const found = i >= last && field(line);
    if (found) keep(found[1].toLowerCase(), found[2].trim());
  });
  return FACTS.flatMap(({ label, keys }) => {
    const key = keys.find((k) => fields.has(k));
    return key ? [{ label, value: fields.get(key)! }] : [];
  });
}

/** A whois answer without the registry's comment lines (terms of use and the like). */
export function whoisRecord(text: string): string {
  const kept = text.split(/\r?\n/).filter((line) => !/^[%#]/.test(line));
  return kept.join("\n").replace(/\n{3,}/g, "\n\n").trim();
}
