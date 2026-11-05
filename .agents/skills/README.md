# BeautifulRust skills

These project-local skills are copied from
[BeautifulRust](https://github.com/morganlinton/BeautifulRust) at commit
`edd3e86087eda07983251056508fe3b8856d85d7` (retrieved 2026-11-05).

- [rust-best-practices](rust-best-practices/SKILL.md): writing and refactoring Rust.
- [rust-review](rust-review/SKILL.md): reviewing Rust code and diffs.
- [rust-port](rust-port/SKILL.md): porting other languages to Rust.

The skill text and reference digests are unchanged. Upstream evaluation fixtures are
omitted; they test agents rather than TracePlotter. The six reference digests remain
with the writing skill so its relative lookups work. The original sources retain
their own licenses; the digests are upstream's paraphrased summaries.

[LICENSE-BeautifulRust](LICENSE-BeautifulRust) contains the upstream MIT notice.
TracePlotter's project guidance in [AGENTS.md](../../AGENTS.md) and
[CONTRIBUTING.md](../../CONTRIBUTING.md) takes precedence, including the native
checks, test placement, module layout and dependency policy.

To update, select and review an upstream commit, replace these three folders with
the corresponding `skills/` folders from that commit, omit `evals/`, and refresh
the license and commit recorded here. Review guideline changes against TracePlotter's
conventions before adopting them. Do not execute upstream evaluation scripts.

Codex and other agents that support project-local `.agents/skills/` can discover
these skills. Agents that do not scan this directory can follow the explicit links
in `AGENTS.md`. New skills are available to Codex on the next turn.
