#!/usr/bin/env python3
"""Tests for the smoke harness itself.

Two things worth automating, because getting either wrong makes the harness lie
rather than fail:

  - the **skipped-agent path**: an uninstalled agent must report `skipped`, never
    `pass`. A green run that quietly tested two of eight agents is worse than no
    run;
  - the **failure path**: pointed at an agent that never delivers, the runner
    must report a failure carrying pane output, and must terminate rather than
    hang waiting for a marker that will never appear.

Everything here is deterministic — no tmux, no agent binaries, no auth.

    python3 tests/smoke/test_agent_smoke.py
"""

import argparse
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import agent_smoke as smoke


def fake_matrix() -> dict:
    return {
        "phases": ["research", "planning", "running", "review"],
        "agents": [
            {
                "name": "claude",
                "binary": "claude",
                "launch_prompt_verified": True,
                "process_names": ["claude"],
                "active_indicators": ["Claude Code"],
                "dialogs": [
                    {
                        "patterns": ["Yes, I trust this folder"],
                        "require_all": False,
                        "answer": "1",
                        "scope": "launch",
                    }
                ],
                "commands": {"agtx": {"planning": "/agtx:plan {task_id}", "review": "/agtx:review"}},
            },
            {
                "name": "ghost",
                "binary": "ghost-cli",
                "launch_prompt_verified": False,
                "process_names": ["ghost-cli"],
                "active_indicators": [],
                "dialogs": [],
                "commands": {"agtx": {"planning": "/agtx-plan {task_id}"}},
            },
        ],
        "plugins": [
            {
                "name": "agtx",
                "supported_agents": [],
                "init_script": None,
                "copy_dirs": [],
                "artifacts": {"planning": ".agtx/plan.md", "running": ".agtx/execute.md"},
                "commands": {"planning": "/agtx:plan {task_id}", "review": "/agtx:review"},
                "prompts": {},
            },
            {
                "name": "gsd",
                "supported_agents": ["claude"],
                "init_script": "npx get-shit-done-cc",
                "copy_dirs": [],
                "artifacts": {"planning": ".planning/phases/*/{phase}-PLAN.md"},
                "commands": {"planning": "/gsd:plan-phase {phase}"},
                "prompts": {"research": "Task: {task}"},
            },
            {
                "name": "void",
                "supported_agents": [],
                "init_script": None,
                "copy_dirs": [],
                "artifacts": {},
                "commands": {},
                "prompts": {},
            },
        ],
    }


def opts(**overrides) -> argparse.Namespace:
    base = dict(
        agents="installed",
        plugins="agtx",
        phases=["planning"],
        agtx_bin="/nonexistent/agtx",
        cargo="cargo",
        workdir=None,
        phase_timeout=1,
        worktree_timeout=1,
        transition_timeout=1,
        keep_sessions=True,
        clean=False,
        force_unsupported=False,
        json_out=None,
        verbose=False,
    )
    base.update(overrides)
    return argparse.Namespace(**base)


def only(which: str):
    """A `shutil.which` that finds exactly one binary."""
    return lambda binary: f"/usr/local/bin/{binary}" if binary == which else None


class PlanCases(unittest.TestCase):
    def test_uninstalled_agent_is_skipped_never_run(self):
        with mock.patch.object(smoke.shutil, "which", only("claude")):
            cases = smoke.plan_cases(fake_matrix(), "claude,ghost", "agtx")
        by_agent = {c.agent: c for c in cases}
        self.assertIsNone(by_agent["claude"].skip_reason)
        self.assertIn("ghost-cli not installed", by_agent["ghost"].skip_reason)

    def test_installed_selection_only_picks_present_binaries(self):
        with mock.patch.object(smoke.shutil, "which", only("claude")):
            cases = smoke.plan_cases(fake_matrix(), "installed", "agtx")
        self.assertEqual([c.agent for c in cases], ["claude"])

    def test_every_excluded_case_still_appears_with_a_reason(self):
        """An exclusion must be a visible row, never an omission."""
        with mock.patch.object(smoke.shutil, "which", only("claude")):
            cases = smoke.plan_cases(fake_matrix(), "all", "all")
        self.assertEqual(len(cases), 6)  # 2 agents x 3 plugins, nothing dropped
        for case in cases:
            if case.skip_reason is None:
                continue
            self.assertTrue(case.skip_reason.strip())

    def test_unsupported_agent_for_plugin_is_skipped(self):
        with mock.patch.object(smoke.shutil, "which", lambda b: "/bin/" + b):
            cases = smoke.plan_cases(fake_matrix(), "ghost", "gsd", force=True)
        self.assertIn("supported_agents", cases[0].skip_reason)

    def test_plugins_needing_setup_are_skipped_unless_forced(self):
        with mock.patch.object(smoke.shutil, "which", only("claude")):
            skipped = smoke.plan_cases(fake_matrix(), "claude", "gsd")
            forced = smoke.plan_cases(fake_matrix(), "claude", "gsd", force=True)
        self.assertIn("init_script", skipped[0].skip_reason)
        self.assertIsNone(forced[0].skip_reason)


class PluginClassification(unittest.TestCase):
    def test_prereqs_are_derived_from_the_plugin_toml(self):
        self.assertIsNone(smoke.plugin_prereq({"commands": {"planning": "/x"}}))
        self.assertIn("init_script", smoke.plugin_prereq({"init_script": "npx foo"}))
        self.assertIn(
            ".specify", smoke.plugin_prereq({"copy_dirs": [".specify"], "commands": {"a": "b"}})
        )
        self.assertIn("nothing", smoke.plugin_prereq({"commands": {}, "prompts": {}}))

    def test_a_phase_without_task_text_is_not_assertable(self):
        plugin = fake_matrix()["plugins"][0]
        self.assertTrue(smoke.phase_is_assertable(plugin, "planning"))
        # agtx sends a bare `/agtx:review`, and clears context on advance — the
        # agent has no way to know what the smoke task asked for.
        self.assertFalse(smoke.phase_is_assertable(plugin, "review"))


class Fixture(unittest.TestCase):
    def test_description_carries_the_real_task_id_per_phase(self):
        desc = smoke.build_description(
            "abc-123", {"planning": ".agtx/plan.md"}, ["planning", "running"]
        )
        self.assertIn("DELIVERED abc-123", desc)
        self.assertIn(".agtx/smoke/planning.txt", desc)
        self.assertIn(".agtx/smoke/running.txt", desc)
        self.assertIn(".agtx/plan.md", desc)

    def test_the_artifact_carries_the_fixture_into_the_next_phase(self):
        """agtx's execute skill reads plan.md and is told NOT to call get_task."""
        desc = smoke.build_description(
            "abc-123", {"planning": ".agtx/plan.md"}, ["planning", "running"]
        )
        self.assertIn(smoke.ARTIFACT_POINTER, desc)
        self.assertIn(smoke.INSTRUCTIONS_REL, smoke.ARTIFACT_POINTER)

    def test_the_fixture_file_repeats_the_phase_list(self):
        with tempfile.TemporaryDirectory() as tmp:
            wt = Path(tmp)
            smoke.write_fixture(wt, "abc-123", {"running": ".agtx/execute.md"}, ["running"])
            text = (wt / smoke.INSTRUCTIONS_REL).read_text()
            self.assertIn("DELIVERED abc-123", text)
            self.assertIn(".agtx/smoke/running.txt", text)

    def test_concrete_artifact_resolves_wildcards_and_phase(self):
        self.assertEqual(
            smoke.concrete_artifact(".planning/phases/*/{phase}-PLAN.md"),
            ".planning/phases/smoke/1-PLAN.md",
        )

    def test_find_artifact_matches_both_phase_spellings_and_globs(self):
        with tempfile.TemporaryDirectory() as tmp:
            wt = Path(tmp)
            (wt / ".planning" / "phases" / "anything").mkdir(parents=True)
            (wt / ".planning" / "phases" / "anything" / "01-PLAN.md").write_text("x")
            self.assertTrue(
                smoke.find_artifact(wt, ".planning/phases/*/{phase}-PLAN.md", 1)
            )
            self.assertFalse(smoke.find_artifact(wt, ".agtx/plan.md", 1))


class Checks(unittest.TestCase):
    def test_marker_must_contain_the_id_that_was_passed_in(self):
        with tempfile.TemporaryDirectory() as tmp:
            wt = Path(tmp)
            self.assertEqual(
                smoke.check_marker(wt, "planning", "abc-123", True).state, "fail"
            )
            marker = wt / smoke.marker_rel("planning")
            marker.parent.mkdir(parents=True)
            marker.write_text("DELIVERED some-other-id\n")
            check = smoke.check_marker(wt, "planning", "abc-123", True)
            self.assertEqual(check.state, "fail")
            self.assertIn("lacks the task id", check.detail)
            marker.write_text("DELIVERED abc-123\n")
            self.assertEqual(
                smoke.check_marker(wt, "planning", "abc-123", True).state, "pass"
            )

    def test_marker_is_not_applicable_when_the_phase_carries_no_task_text(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(
                smoke.check_marker(Path(tmp), "review", "abc", False).state, "n/a"
            )

    def test_command_parked_in_a_composer_is_a_failure(self):
        pane = "\n".join(["welcome", "", "> /agtx:plan abc-123"])
        check = smoke.check_command_submitted(pane, "/agtx:plan abc-123", False)
        self.assertEqual(check.state, "fail")
        self.assertIn("nothing ran", check.detail)

    def test_command_at_the_bottom_is_not_a_park_when_work_landed(self):
        """Claude Code renders a dim suggestion in its empty composer."""
        pane = "\n".join(["wrote the files", "", "> /agtx:execute abc-123"])
        self.assertEqual(
            smoke.check_command_submitted(pane, "/agtx:execute abc-123", True).state,
            "pass",
        )

    def test_command_with_output_under_it_counts_as_submitted(self):
        pane = "\n".join(
            ["> /agtx:plan abc-123", "", "Reading the task…", "Wrote .agtx/plan.md", "❯"]
        )
        self.assertEqual(
            smoke.check_command_submitted(pane, "/agtx:plan abc-123", False).state, "pass"
        )

    def test_invisible_command_is_unknown_not_a_failure(self):
        """Several TUIs redraw their scrollback; check 2 catches real misses."""
        self.assertEqual(
            smoke.check_command_submitted("nothing here", "/agtx:plan abc", False).state,
            "unknown",
        )

    def test_agent_without_command_syntax_is_not_applicable(self):
        self.assertEqual(smoke.check_command_submitted("x", None, False).state, "n/a")

    def test_dialog_is_matched_only_on_the_visible_tail(self):
        dialogs = [
            dict(d, owner="claude") for d in fake_matrix()["agents"][0]["dialogs"]
        ]
        on_screen = "some output\nYes, I trust this folder"
        self.assertIsNotNone(smoke.visible_dialog(on_screen, dialogs))
        scrolled_away = "Yes, I trust this folder\n" + "\n".join(
            f"line {i}" for i in range(60)
        )
        self.assertIsNone(smoke.visible_dialog(scrolled_away, dialogs))

    def test_dialogs_agtx_does_not_answer_are_still_caught(self):
        """The antigravity trap: no agent declares it, the session still parks.

        Antigravity's own wording is no longer the example here — this runner is
        what got it declared. What the check still has to catch is a prompt that
        belongs to no agent's spec at all.
        """
        self.assertIn(
            "unhandled",
            smoke.visible_dialog("Continue? Press Enter to continue", []),
        )

    def test_a_declared_dialog_is_no_longer_listed_as_unhandled(self):
        """Guards the contradiction directly: a pattern cannot be both.

        The only test here that needs the crate, since the declared side comes
        from `AGENT_SPECS`. Skipped rather than failed without a toolchain, so
        the rest stay runnable anywhere.
        """
        if not shutil.which("cargo"):
            self.skipTest("cargo not available")
        try:
            matrix = smoke.load_matrix()
        except (RuntimeError, OSError) as exc:
            self.skipTest(f"could not load the agent matrix: {exc}")
        declared = {
            p for a in matrix["agents"] for d in a["dialogs"] for p in d["patterns"]
        }
        overlap = declared & set(smoke.UNHANDLED_DIALOG_PATTERNS)
        self.assertEqual(
            overlap,
            set(),
            "a dialog agtx answers must not also be listed as unhandled",
        )

    def test_usable_requires_a_live_process_and_a_clear_screen(self):
        agent = fake_matrix()["agents"][0]
        with mock.patch.object(smoke, "pane_command", return_value="claude"), \
                mock.patch.object(smoke, "pane_pid", return_value=None):
            self.assertEqual(
                smoke.check_usable("t:1", "all good", agent, [], None).state, "pass"
            )
            self.assertEqual(
                smoke.check_usable("t:1", "ok", agent, [], "blocked", False).state, "fail"
            )
            # A finished phase reports `blocked` too: agtx maps Claude's
            # Notification event, which also means "waiting for your input".
            self.assertEqual(
                smoke.check_usable("t:1", "ok", agent, [], "blocked", True).state, "pass"
            )
        with mock.patch.object(smoke, "pane_command", return_value="zsh"), \
                mock.patch.object(smoke, "pane_pid", return_value=4242), \
                mock.patch.object(smoke, "process_tree", return_value=["sh", "zsh"]):
            check = smoke.check_usable("t:1", "no indicators", agent, [], None)
            self.assertEqual(check.state, "fail")
            self.assertIn("agent process is gone", check.detail)

    def test_a_wrapper_shell_does_not_hide_a_live_agent(self):
        """`pane_current_command` reports `bash` for a live Claude pane."""
        agent = fake_matrix()["agents"][0]
        with mock.patch.object(smoke, "pane_command", return_value="bash"), \
                mock.patch.object(smoke, "pane_pid", return_value=4242), \
                mock.patch.object(smoke, "process_tree", return_value=["sh", "claude"]):
            self.assertEqual(
                smoke.check_usable("t:1", "no indicators here", agent, [], None).state,
                "pass",
            )

    def test_an_agent_shipped_as_a_node_script_counts_as_live(self):
        agent = fake_matrix()["agents"][1]
        with mock.patch.object(smoke, "pane_command", return_value="bash"), \
                mock.patch.object(smoke, "pane_pid", return_value=4242), \
                mock.patch.object(smoke, "process_tree", return_value=["sh", "node"]):
            self.assertEqual(
                smoke.check_usable("t:1", "", agent, [], None).state, "pass"
            )


class ProcessTree(unittest.TestCase):
    def test_the_ps_walk_finds_this_very_process(self):
        """Guards the `ps` parsing itself — the format differs across platforms."""
        tree = smoke.process_tree(os.getpid())
        self.assertTrue(
            any("python" in name for name in tree),
            f"expected a python process in {tree}",
        )

    def test_a_pid_with_no_processes_yields_nothing(self):
        self.assertEqual(smoke.process_tree(0x7FFFFFFF), [])


class StubMcp:
    """Just enough MCP for drive_phase: a fixed pane and a fixed task record."""

    def __init__(self, pane: str, task: dict | None = None):
        self.pane = pane
        self.task = task or {"session_name": "smoke:task", "status": "planning"}

    def call(self, tool: str, **kwargs):
        if tool == "read_pane_content":
            return {"content": self.pane}
        if tool == "get_task":
            return self.task
        raise AssertionError(f"unexpected tool {tool}")


class FailurePath(unittest.TestCase):
    """Pointed at an agent that never delivers, the runner must report, not hang."""

    def runner(self, pane: str, **opt_overrides) -> smoke.CaseRunner:
        case = smoke.Case("claude", "agtx")
        r = smoke.CaseRunner(case, fake_matrix(), opts(**opt_overrides), Path("/tmp"))
        r.mcp = StubMcp(pane)
        return r

    def test_undelivered_prompt_fails_with_pane_output(self):
        pane = "\n".join(["claude starting", "", "> /agtx:plan abc-123"])
        r = self.runner(pane)
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(smoke.time, "sleep"), \
                mock.patch.object(smoke, "pane_command", return_value="claude"), \
                mock.patch.object(smoke, "pane_pid", return_value=None):
            result = r.drive_phase("abc-123", "planning", Path(tmp), 1)
        self.assertEqual(result.outcome, smoke.FAIL)
        self.assertTrue(result.pane_tail, "a failure must carry pane output")
        self.assertIn("/agtx:plan abc-123", result.pane_tail)
        states = {c.name: c.state for c in result.checks}
        self.assertEqual(states["skill-ran"], "fail")
        self.assertEqual(states["artifact"], "fail")

    def test_a_parked_dialog_is_reported_not_waited_out(self):
        r = self.runner("Yes, I trust this folder", phase_timeout=600)
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(smoke.time, "sleep"), \
                mock.patch.object(smoke, "pane_command", return_value="claude"), \
                mock.patch.object(smoke, "pane_pid", return_value=None):
            result = r.drive_phase("abc-123", "planning", Path(tmp), 1)
        self.assertEqual(result.outcome, smoke.FAIL)
        usable = next(c for c in result.checks if c.name == "usable")
        self.assertEqual(usable.state, "fail")
        self.assertIn("dialog on screen", usable.detail)

    def test_usage_limit_is_reported_separately_from_a_failure(self):
        r = self.runner("You've hit your session limit", phase_timeout=600)
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(smoke.time, "sleep"), \
                mock.patch.object(smoke, "pane_command", return_value="claude"), \
                mock.patch.object(smoke, "pane_pid", return_value=None):
            result = r.drive_phase("abc-123", "planning", Path(tmp), 1)
        self.assertEqual(result.outcome, smoke.QUOTA)

    def test_a_credential_error_short_circuits_as_auth(self):
        r = self.runner("AWS SigV4 authentication requires AWS credentials.",
                        phase_timeout=600)
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(smoke.time, "sleep"), \
                mock.patch.object(smoke, "pane_command", return_value="opencode"), \
                mock.patch.object(smoke, "pane_pid", return_value=None):
            result = r.drive_phase("abc-123", "planning", Path(tmp), 1)
        self.assertEqual(result.outcome, smoke.AUTH)

    def test_a_complete_phase_passes_every_check(self):
        pane = "\n".join(
            ["> /agtx:plan abc-123", "", "wrote the files", "❯"]
        )
        r = self.runner(pane, phase_timeout=600)
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(smoke.time, "sleep"), \
                mock.patch.object(smoke, "pane_command", return_value="claude"), \
                mock.patch.object(smoke, "pane_pid", return_value=None):
            wt = Path(tmp)
            marker = wt / smoke.marker_rel("planning")
            marker.parent.mkdir(parents=True)
            marker.write_text("DELIVERED abc-123")
            (wt / ".agtx" / "plan.md").write_text("smoke")
            result = r.drive_phase("abc-123", "planning", wt, 1)
        self.assertEqual(result.outcome, smoke.PASS)
        self.assertEqual([c.state for c in result.checks], ["pass"] * 4)


class QuotaPatterns(unittest.TestCase):
    def test_an_informational_tip_is_not_a_quota_park(self):
        """Codex prints this at startup on a perfectly healthy session."""
        self.assertIsNone(
            smoke.match_any(
                smoke.QUOTA_PATTERNS,
                "• You have 1 usage limit reset available. Run /usage to use one.",
            )
        )

    def test_a_real_park_is_still_caught(self):
        for line in (
            "You've hit your session limit. Press Enter to continue after reset",
            "usage limit reached",
            # Grok's wording — no apostrophe, and a word in the middle. It was
            # reported as a send-path FAIL until the pattern was widened.
            "You hit your free usage limit.",
        ):
            self.assertIsNotNone(smoke.match_any(smoke.QUOTA_PATTERNS, line), line)


class AuthPatterns(unittest.TestCase):
    def test_a_provider_credential_error_is_classified_as_auth(self):
        """It looks exactly like an undelivered prompt from the outside."""
        for line in (
            "AWS SigV4 authentication requires AWS credentials. Please provide either:",
            "Original error: AWS access key ID setting is missing.",
            "Opening authentication page in your browser.",
            '"message": "API key not valid. Please pass a valid API key."',
        ):
            self.assertIsNotNone(smoke.match_any(smoke.AUTH_PATTERNS, line), line)

    def test_ordinary_output_is_not_an_auth_prompt(self):
        self.assertIsNone(
            smoke.match_any(smoke.AUTH_PATTERNS, "Wrote .agtx/plan.md and stopped.")
        )

    def test_a_generic_failure_summary_is_not_laundered_into_auth(self):
        """Gemini prints this next to a real credential error and to any other."""
        self.assertIsNone(
            smoke.match_any(
                smoke.AUTH_PATTERNS,
                "This request failed. Press F12 for diagnostics",
            )
        )


class ParkExpectation(unittest.TestCase):
    """The set is empty today, so every case here installs its own entry.

    The mechanism stays tested rather than deleted: the next agent that parks on
    a dialog agtx cannot answer needs it, and its guards — never excusing a lost
    prompt, and flagging itself stale — are the whole reason it is safe to have.
    """

    def setUp(self):
        patcher = mock.patch.object(smoke, "EXPECTED_PARK_AGENTS", {"antigravity"})
        patcher.start()
        self.addCleanup(patcher.stop)

    def runner(self, agent: str) -> smoke.CaseRunner:
        matrix = fake_matrix()
        matrix["agents"][1]["name"] = agent
        return smoke.CaseRunner(smoke.Case(agent, "agtx"), matrix, opts(), Path("/tmp"))

    def result(self, agent: str, outcome: str, usable_state: str) -> smoke.CaseResult:
        phase = smoke.PhaseResult("planning")
        phase.checks = [
            smoke.Check("submitted", "pass"),
            smoke.Check("skill-ran", "pass"),
            smoke.Check("artifact", "pass"),
            smoke.Check("usable", usable_state, "dialog on screen — unhandled: trust"),
        ]
        return smoke.CaseResult(agent, "agtx", outcome, phases=[phase])

    def test_delivered_then_parked_is_its_own_outcome(self):
        r = self.runner("antigravity")
        out = r.apply_park_expectation(self.result("antigravity", smoke.FAIL, "fail"))
        self.assertEqual(out.outcome, smoke.PARK)
        self.assertIn("delivered, then parked", out.reason)

    def test_a_clean_pass_flags_the_expectation_as_stale(self):
        r = self.runner("antigravity")
        out = r.apply_park_expectation(self.result("antigravity", smoke.PASS, "pass"))
        self.assertEqual(out.outcome, smoke.PASS)
        self.assertIn("stale", out.reason)

    def test_an_undelivered_prompt_is_never_excused_as_a_park(self):
        """The park expectation covers a blocked session, not a lost prompt."""
        r = self.runner("antigravity")
        res = self.result("antigravity", smoke.FAIL, "fail")
        res.phases[0].checks[1] = smoke.Check("skill-ran", "fail", "never written")
        self.assertEqual(r.apply_park_expectation(res).outcome, smoke.FAIL)

    def test_other_agents_are_unaffected(self):
        r = self.runner("ghost")
        out = r.apply_park_expectation(self.result("ghost", smoke.FAIL, "fail"))
        self.assertEqual(out.outcome, smoke.FAIL)


class Args(unittest.TestCase):
    def test_phases_must_be_in_workflow_order(self):
        with self.assertRaises(SystemExit):
            smoke.parse_args(["--phases", "running,planning"])
        with self.assertRaises(SystemExit):
            smoke.parse_args(["--phases", "backlog"])
        self.assertEqual(
            smoke.parse_args(["--phases", "planning,review"]).phases,
            ["planning", "review"],
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
