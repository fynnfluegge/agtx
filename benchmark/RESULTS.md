# Benchmark Results

Tracks agtx benchmark runs against [SWE-bench Lite](https://github.com/princeton-nlp/SWE-bench).
All runs use the `agtx` workflow plugin with Claude Sonnet as the agent, sandbox mode (`--sandbox`).

> **Note on tokens vs cost:** Token count is a simple sum of all token types (input + output + cache read + cache write). Since cache reads are 10× cheaper than regular input ($0.30 vs $3.00/MTok on Sonnet 4.6), a run with more tokens can have lower cost if those extra tokens are mostly cache reads. Conversely, compression tools that save output tokens ($15/MTok) reduce cost disproportionately relative to their token savings.

## astropy__astropy-12907

| Config | Date | Duration | Tokens | Cost | Resolved | Notes |
|--------|------|----------|--------|------|----------|-------|
| `claude-agtx` | 2026-06-29 | 2m 38s | 1,514K | $1.00 | ❌ | Correct fix in `separable.py` but 2/4 target tests still fail (`compound_model6`, `compound_model9`) |
| `claude-rtk-caveman-agtx` | 2026-06-29 | 2m 14s | 1,149K | $0.92 | ❌ | Correct fix in `separable.py` but 2/4 target tests still fail (`compound_model6`, `compound_model9`) |
| `claude-rtk-caveman-ponytail-agtx` | 2026-06-29 | 2m 09s | 1,006K | $0.55 | ❌ | Correct fix in `separable.py` but 2/4 target tests still fail (`compound_model6`, `compound_model9`) |
| `claude-rtk-ponytail-agtx` | 2026-06-30 | 2m 27s | 1,314K | $1.02 | ❌ | Correct fix in `separable.py` but 2/4 target tests still fail (`compound_model6`, `compound_model9`) |
| `claude-superpowers` | 2026-06-30 | 15m 46s | 5,400K | $2.64 | ✅ | Correct fix + test case added; resolved in 1/2 runs (nondeterministic test key name) |
| `claude-spec-kit` | 2026-06-30 | 4m 48s | 926K | $0.40 | ❌ | Correct fix in `separable.py`; no test case added so harness fails |

## astropy__astropy-14182

| Config | Date | Duration | Tokens | Cost | Resolved | Notes |
|--------|------|----------|--------|------|----------|-------|
| `claude-agtx` | 2026-06-29 | 3m 17s | 1,400K | $1.11 | ❌ | Fix attempted but tests still fail |
| `claude-rtk-caveman-agtx` | 2026-06-29 | 3m 31s | 1,800K | $1.18 | ❌ | Fix attempted but tests still fail |
| `claude-rtk-caveman-ponytail-agtx` | 2026-06-29 | 6m 07s | 2,500K | $1.15 | ❌ | Fix attempted but tests still fail |
| `claude-rtk-ponytail-agtx` | 2026-06-30 | 5m 21s | 2,586K | $1.21 | ❌ | Fix attempted but tests still fail |

**Notes:**
- Compression tools showed no benefit on this task — tokens and cost were higher than baseline
- Ponytail run took twice as long (6m 07s vs 3m 17s), suggesting the agent explored more paths
- Single-task comparisons are noisy; trends are more reliable across many instances
