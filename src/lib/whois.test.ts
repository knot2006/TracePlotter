import assert from "node:assert/strict";
import { test } from "node:test";
import { whoisFacts, whoisRecord } from "./whois.ts";

const RIPE = `% This is the RIPE Database query service.
%
% Abuse contact for '192.0.2.0 - 192.0.2.255' is 'abuse@example.net'

inetnum:        192.0.2.0 - 192.0.2.255
netname:        EXAMPLE
descr:          Example Telecom
descr:          Street 13
country:        PL


route:          192.0.2.0/24
origin:         AS64500
`;

const ARIN = `# ARIN WHOIS data and services are subject to the Terms of Use
NetRange:       198.51.100.0 - 198.51.100.255
CIDR:           198.51.100.0/24
NetName:        EXAMPLE-NET
OriginAS:       AS64501
OrgName:        Example LLC
Country:        US
OrgAbuseEmail:  abuse@example.com
`;

test("whoisFacts reads the fields of different registries", () => {
  assert.deepEqual(whoisFacts(RIPE), [
    { label: "Organisation", value: "Example Telecom" },
    { label: "Network", value: "EXAMPLE" },
    { label: "Range", value: "192.0.2.0 - 192.0.2.255" },
    { label: "Route", value: "192.0.2.0/24" },
    { label: "AS number", value: "AS64500" },
    { label: "Country", value: "PL" },
    { label: "Abuse contact", value: "abuse@example.net" },
  ]);
  const arin = Object.fromEntries(whoisFacts(ARIN).map((fact) => [fact.label, fact.value]));
  assert.equal(arin.Organisation, "Example LLC");
  assert.equal(arin.Route, "198.51.100.0/24");
  assert.equal(arin["Abuse contact"], "abuse@example.com");
  assert.deepEqual(whoisFacts("Lookup failed: timed out"), []);
});

test("whoisRecord drops comment lines and runs of blank lines", () => {
  const record = whoisRecord(RIPE);
  assert.ok(record.startsWith("inetnum:"));
  assert.ok(!record.includes("%"));
  assert.ok(!record.includes("\n\n\n"));
});
