---
name: agtx-review
description: Self-review completed work. Check for correctness, edge cases, and code quality. Write review to .agtx/review.md and stop.
---

# Review Phase

You are in the **review phase** of an agtx-managed task.

## Instructions

1. Review all changes made during execution (use git diff)
2. Check for:
   - Correctness and edge cases
   - Error handling
   - Code style consistency with the existing codebase
   - Test coverage
   - Security issues (injection, XSS, etc.)
3. Fix any issues you find

## Output

Write your review to `.agtx/review.md` with these sections:

## Review
Findings from your review — what looks good, what was fixed, any concerns.

## Status
Either `READY` (good to merge) or `NEEDS_WORK` (with explanation of remaining issues).

## CRITICAL: Stop After Writing

After writing `.agtx/review.md`:
- Say: "Review written to `.agtx/review.md`."
- Wait for further instructions
