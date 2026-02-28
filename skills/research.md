---
name: agtx-research
description: Explore the codebase to understand a task before planning. Write findings to .agtx/research/{task-id}.md and stop. This is a read-only exploration — do not modify any files.
---

# Research Phase

You are in the **research phase** of an agtx-managed task. This is a read-only exploration.

## Instructions

1. **Wait for the next message** — it contains the task title, description, and artifact path. Do not start working until you receive it.
2. Read and understand the task description
3. Explore the codebase to find relevant files, patterns, and architecture
4. Identify dependencies, related code, and potential complexity
5. Assess feasibility and estimate scope

## Output

Write your findings to the research artifact path provided in the task prompt. Include:

## Relevant Files
Key files and their roles — what exists, what needs changing.

## Architecture
How the relevant parts of the codebase fit together.

## Complexity
Assessment of scope — simple change, moderate refactor, or major undertaking.

## Open Questions
Things that need clarification before planning can begin.

## CRITICAL: Do Not Modify Code

This is a **read-only** exploration:
- Do NOT modify any source files
- Do NOT create branches or worktrees
- Do NOT start planning or implementing
- Say: "Research complete. Findings written to {artifact_path}."
- Wait for further instructions
