# Benchmark Results

Tracks agtx benchmark runs against [SWE-bench Lite](https://github.com/princeton-nlp/SWE-bench).
All runs use Claude Sonnet as the agent, sandbox mode (`--sandbox`).

> **Note on tokens vs cost:** Token count is a simple sum of all token types (input + output + cache read + cache write). Since cache reads are 10× cheaper than regular input ($0.30 vs $3.00/MTok on Sonnet 4.6), a run with more tokens can have lower cost if those extra tokens are mostly cache reads.
>
> **Resolution icons:** ✅ Resolved — 🟡 Fix correct but incomplete (missing tests, spurious changes) — ❌ Fix wrong or no patch

---

## `claude-agtx`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-29 | 2m 38s | 1,514K | $1.00 | 🟡 | Correct fix in `separable.py`; missing test cases |
| `astropy__astropy-14182` | 2026-06-29 | 3m 17s | 1,400K | $1.11 | ❌ | Fix attempted but tests still fail |

## `claude-rtk-caveman-agtx`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-29 | 2m 14s | 1,149K | $0.92 | 🟡 | Correct fix in `separable.py`; missing test cases |
| `astropy__astropy-14182` | 2026-06-29 | 3m 31s | 1,800K | $1.18 | ❌ | Fix attempted but tests still fail |

## `claude-rtk-ponytail-agtx`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-30 | 2m 27s | 1,314K | $1.02 | 🟡 | Correct fix in `separable.py`; missing test cases |
| `astropy__astropy-14182` | 2026-06-30 | 5m 21s | 2,586K | $1.21 | ❌ | Fix attempted but tests still fail |

## `claude-rtk-caveman-ponytail-agtx`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-29 | 2m 09s | 1,006K | $0.55 | 🟡 | Correct fix in `separable.py`; missing test cases |
| `astropy__astropy-14182` | 2026-06-29 | 6m 07s | 2,500K | $1.15 | ❌ | Fix attempted but tests still fail |

## `claude-superpowers`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-30 | 15m 46s | 5,400K | $2.64 | ✅ | Correct fix + test case added; |

## `claude-spec-kit`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-30 | 4m 48s | 926K | $0.40 | 🟡 | Correct fix in `separable.py`; no test case added |

## `claude-openspec`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-06-30 | 2m 15s | 468K | $0.48 | 🟡 | Correct fix in `separable.py`; no test case added; `/opsx:verify` command didn't exist (fixed) |

## `claude-agent-skills`

| Instance | Date | Duration | Tokens | Cost | Result | Notes |
|----------|------|----------|--------|------|--------|-------|
| `astropy__astropy-12907` | 2026-07-01 | 3m 30s | 2,680K | $1.04 | 🟡 | Correct fix in `separable.py`; no test case added; spurious `setuptools==68.0.0` pin |
