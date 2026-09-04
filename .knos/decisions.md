# Decisions and current work

<!-- Written by `knos export`. Commit this file. -->

<!--
Reading this file needs nothing installed: it is plain markdown, and a fresh
clone picks it up as-is. The live claim/withhold server is a separate, optional
step - `pip install knos` (Python 3.10+), which the MCP entry launches as
`python -m knos.mcp`. Without it, everything below still reads normally.
-->


A second clone reads this on its first question — it is one of the decision
records knos looks for. Nothing here is private: secrets and private paths
never reach it.


## Decisions

- **task lifecycle** — Tasks move through `Backlog → Planning → Running → Review → Done`. There is no separate Research status; Backlog's display name is `backlog/research`.  _(AGENTS.md, CLAUDE.md)_
- **a worktree per task** — Planning creates a git worktree at `{worktree_dir}/{slug}`, default `.agtx/worktrees/{slug}`, copies configured files, runs the init script, deploys skills and starts the agent in planning mode.  _(CLAUDE.md)_
- **an agent per phase** — Different agents are configured per workflow phase, with automatic switching between them.  _(README.md)_
- **parallel by default** — Every task gets its own worktree and tmux window, so as many agents run at once as needed.  _(README.md)_

## Being worked on right now

_Nothing claimed._

---
<sub>knos export. Claims lapse after 30 minutes.</sub>
