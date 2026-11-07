# CLAUDE.md

Guidance for Claude Code in this repository. Specialist subagents (`architect`, `implementer`, `tester`) are defined in `.claude/agents/`; this file is ported from `.cursor/rules/engineering-workflow.mdc` — keep the two in sync.

A fourth agent, `reviewer`, is **not** part of this workflow. Invoke it only when the user asks for an architecture review. It writes documentation only (Markdown under `docs/`, enforced by `.claude/hooks/docs-only-writes.py`), saving change proposals to `docs/reviews/` for the implementer. Its report ends with open questions; relay them to the user and resume the reviewer with the answers.

## Engineering Workflow

For substantial gameplay engineering tasks, follow this workflow.

### Architecture

1. Delegate investigation and design to the architect subagent.
2. Require the architect to inspect existing implementations and identify reusable systems.
3. Require an implementation specification under docs/plans/. The architect writes it as `docs/plans/YYYY-MM-DD-<topic>.md`. It can write only Markdown under `docs/`, enforced by `.claude/hooks/docs-only-writes.py`.
4. Update permanent architecture documentation only when the architecture meaningfully changes.

### Implementation

1. Delegate the approved specification to the implementer subagent.
2. Require incremental implementation and appropriate regression tests.
3. Require implementation documentation for new reusable mechanics or meaningful changes to existing mechanics.
4. Ensure deviations from the approved architecture are justified.

### Verification

1. Delegate independent verification to the tester subagent.
2. Require inspection of the actual code changes and execution of relevant tests.
3. If defects are found, delegate corrections to the implementer.
4. Require targeted re-verification.

### Efficiency

- Do not invoke every specialist for simple tasks.
- Avoid repeated investigation of the same files.
- Pass specification paths and relevant findings between agents.
- Reuse existing documentation.
- Avoid unnecessary parallel implementation of shared files.

### Sequential execution

The coordinator must not start implementer until architect work is complete and the plan is approved. Do not start tester until implementer work is complete (PR or branch ready for verification). One specialist at a time; no parallel architect/implementer/tester on the same workstream.

### Single pull request

For one gameplay workstream, architect, implementer, and tester share the same branch and pull request. The coordinator may open the PR with the architect's plan; implementer and tester commit to that branch. Do not open parallel PRs for plan vs implementation vs verification.

### Coordinator announcements

When delegating each phase, the main Claude Code session (coordinator) must tell the user the subagent name and the `model` field from that subagent's `.claude/agents/*.md` frontmatter before or as that worker starts.
