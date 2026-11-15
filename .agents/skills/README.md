# BeautifulRust skills

These project-local skills are copied from
[BeautifulRust](https://github.com/morganlinton/BeautifulRust) at commit
`b9718436d77ee7bcf17cdcb6aa27628feb95d97a` (retrieved 2026-11-15).

- [rust-best-practices](rust-best-practices/SKILL.md): writing and refactoring Rust.
- [rust-review](rust-review/SKILL.md): reviewing Rust code and diffs.
- [rust-port](rust-port/SKILL.md): porting other languages to Rust.

The skill text and reference digests are unchanged. Upstream evaluation fixtures are
omitted; they test agents rather than TracePlotter. The reference digests remain
with the writing skill so its relative lookups work. The original sources retain
their own licenses; the digests are upstream's paraphrased summaries.

[LICENSE-BeautifulRust](LICENSE-BeautifulRust) contains the upstream MIT notice.
TracePlotter's project guidance in [AGENTS.md](../../AGENTS.md) and
[CONTRIBUTING.md](../../CONTRIBUTING.md) takes precedence, including the native
checks, test placement, module layout and dependency policy.

The [update workflow](../../.github/workflows/beautifulrust.yml) checks upstream every
Monday at 07:23 UTC and can also be run manually. It opens or refreshes one pull request
when the skills, reference digests or license change. Unrelated upstream commits do not
produce updates. The workflow uses the repository's GitHub token; enable **Allow GitHub
Actions to create and approve pull requests** under **Settings > Actions > General**.
GitHub may require approval of the resulting pull request's CI runs.

To update locally with Node.js 26, run `node scripts/update-beautifulrust.mjs` from the
repository root, optionally followed by a commit or tag. The script resolves that ref
once, verifies each download against the commit's Git blob hash, and updates these
three skills, their Markdown references, the license and the source record above.
It omits `evals/` and stops if upstream introduces an unsupported file layout.

Review guideline changes against TracePlotter's conventions before merging. AGENTS.md,
CONTRIBUTING.md and Cargo lints are maintained separately. Do not execute upstream
evaluation scripts.

Codex and other agents that support project-local `.agents/skills/` can discover
these skills. Agents that do not scan this directory can follow the explicit links
in `AGENTS.md`. New skills are available to Codex on the next turn.
