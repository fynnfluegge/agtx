---
description: Sweep this conversation into agtx tasks and push them to the kanban board
argument-hint: Optional focus area (e.g. "backend tasks only")
allowed-tools: ["mcp__plugin_agtx_agtx__list_tasks", "mcp__plugin_agtx_agtx__create_task", "mcp__plugin_agtx_agtx__create_tasks_batch", "mcp__plugin_agtx_agtx__get_task"]
---

Sweep the conversation into agtx kanban tasks. $ARGUMENTS

Use the sweep skill for guidance. Follow the sweep workflow:
1. Call list_tasks to check for duplicates
2. Extract actionable work items from the conversation
3. Present proposed task list for user confirmation
4. Push to board with create_tasks_batch (or create_task for a single task)
5. Report created task IDs
