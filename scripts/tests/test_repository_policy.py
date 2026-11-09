"""Exercise the CLI and Git hooks using synthetic, temporary repositories only."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SOURCE_ROOT = Path(__file__).resolve().parents[2]
GUARD = SOURCE_ROOT / "scripts" / "check_repository.py"


class RepositoryPolicyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="repository-policy-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "work tree caf\u00e9"
        self.repo.mkdir()
        # Do not inherit Git overrides, hooks, signing, or user-level config.
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        self.env.update({
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "Policy Test",
            "GIT_AUTHOR_EMAIL": "policy@example.invalid",
            "GIT_COMMITTER_NAME": "Policy Test",
            "GIT_COMMITTER_EMAIL": "policy@example.invalid",
            "GIT_TERMINAL_PROMPT": "0",
            "LC_ALL": "C",
        })
        self.git("init", "--quiet", "--initial-branch=feature")
        self.git("config", "commit.gpgsign", "false")

    def run_command(self, command, *, cwd=None, input=None):
        return subprocess.run(
            command, cwd=cwd or self.repo, env=self.env, input=input,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            encoding="utf-8", errors="backslashreplace", check=False,
        )

    def git(self, *args, cwd=None):
        result = self.run_command(["git", *args], cwd=cwd)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def put(self, path, text="synthetic fixture\n", *, repo=None):
        destination = (repo or self.repo) / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")

    def guard(self, *args, cwd=None):
        return self.run_command([sys.executable, str(GUARD), *args], cwd=cwd)

    def assert_passes(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)

    def assert_rejects(self, result, text):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(text, result.stderr)

    def baseline(self, *, agents=False):
        self.put("src/main.rs", "fn main() {}\n")
        self.git("add", "--", "src/main.rs")
        if agents:
            self.put("AGENTS.md", "Synthetic protected fixture, not repository instructions.\n")
            self.git("add", "--", "AGENTS.md")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic baseline")
        return self.git("rev-parse", "HEAD")

    def install_hooks(self):
        for name in ("pre-commit", "pre-push"):
            destination = self.repo / ".githooks" / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(SOURCE_ROOT / ".githooks" / name, destination)
            destination.chmod(destination.stat().st_mode | 0o111)
        destination = self.repo / "scripts" / "check_repository.py"
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(GUARD, destination)
        self.git("config", "core.hooksPath", ".githooks")

    def test_safe_tracked_code_passes_default_and_staged(self):
        self.put("src/main.rs", "fn main() {}\n")
        self.git("add", "--", "src/main.rs")
        self.assert_passes(self.guard())
        self.assert_passes(self.guard("--staged"))

    def test_forced_secret_variants_are_rejected_without_contents(self):
        paths = (".env", ".env.example", "nested/.env.production")
        self.put(".gitignore", ".env\n.env.*\n")
        marker = "SYNTHETIC_SECRET_MUST_NOT_BE_PRINTED"
        for path in paths:
            self.put(path, marker)
            self.git("add", "--force", "--", path)
        for args in ((), ("--staged",)):
            result = self.guard(*args)
            for path in paths:
                self.assert_rejects(result, ascii(path))
            self.assertNotIn(marker, result.stdout + result.stderr)

    def test_generated_and_local_paths_at_root_and_nested_are_rejected(self):
        paths = (
            ".lake/build/object", "nested/.lake/build/object",
            "target/debug/program", "nested/target/debug/program",
            "runs/output.json", "nested/runs/output.json",
            ".worktrees/branch/source.rs", "nested/.worktrees/branch/source.rs",
            "docs/stray/note.txt", "nested/docs/outdated/note.txt",
            "__pycache__/module.cpython-313.pyc", "nested/__pycache__/cache",
            "module.pyc", "nested/module.pyo", "nested/module.pyd",
        )
        for path in paths:
            self.put(path)
            self.git("add", "--force", "--", path)
        result = self.guard()
        for path in paths:
            self.assert_rejects(result, ascii(path))

    def test_safe_similar_names_are_allowed(self):
        paths = ("src/targeting.rs", "src/runtime.rs", "docs/strays.txt", "env.example")
        for path in paths:
            self.put(path)
            self.git("add", "--", path)
        self.assert_passes(self.guard())

    def test_spaces_unicode_quotes_and_newlines_are_not_split(self):
        safe = 'src/a space "caf\u00e9"\nline.rs'
        forbidden = 'a space "caf\u00e9"\nline/.env.local'
        self.put(safe)
        self.git("add", "--", safe)
        self.assert_passes(self.guard("--staged"))
        self.put(forbidden)
        self.git("add", "--force", "--", forbidden)
        result = self.guard("--staged")
        self.assert_rejects(result, ascii(forbidden))
        self.assertEqual(result.stderr.count("environment files"), 1)

    def test_subdirectory_checks_entire_index(self):
        self.put(".env.local")
        self.git("add", "--force", "--", ".env.local")
        subdirectory = self.repo / "src"
        subdirectory.mkdir()
        self.assert_rejects(self.guard(cwd=subdirectory), "'.env.local'")

    def test_staged_agents_modification_is_rejected(self):
        self.baseline(agents=True)
        self.put("AGENTS.md", "Synthetic changed fixture.\n")
        self.git("add", "--", "AGENTS.md")
        self.assert_rejects(self.guard("--staged"), "'AGENTS.md'")

    def test_staged_agents_deletion_is_rejected(self):
        self.baseline(agents=True)
        (self.repo / "AGENTS.md").unlink()
        self.git("add", "--update", "--", "AGENTS.md")
        self.assert_rejects(self.guard("--staged"), "'AGENTS.md'")

    def test_staged_agents_rename_is_rejected(self):
        self.baseline(agents=True)
        self.git("mv", "AGENTS.md", "RENAMED.txt")
        self.assert_rejects(self.guard("--staged"), "'AGENTS.md'")

    def test_staged_nested_agents_addition_is_rejected(self):
        self.put("nested/AGENTS.md", "Synthetic protected fixture.\n")
        self.git("add", "--", "nested/AGENTS.md")
        self.assert_rejects(self.guard("--staged"), "'nested/AGENTS.md'")

    def test_committed_secret_bypassing_hook_is_rejected(self):
        self.baseline()
        self.put(".env.local", "SYNTHETIC_SECRET_MUST_NOT_BE_PRINTED")
        self.git("add", "--force", "--", ".env.local")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic forbidden path")
        self.assert_rejects(self.guard(), "'.env.local'")

    def test_base_detects_agents_change_bypassing_hook(self):
        base = self.baseline(agents=True)
        self.put("AGENTS.md", "Synthetic changed fixture.\n")
        self.git("add", "--", "AGENTS.md")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic protected change")
        self.assert_rejects(self.guard("--base", base), "'AGENTS.md'")

    def test_base_detects_agents_deletion(self):
        base = self.baseline(agents=True)
        (self.repo / "AGENTS.md").unlink()
        self.git("add", "--update", "--", "AGENTS.md")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic protected deletion")
        self.assert_rejects(self.guard("--base", base), "'AGENTS.md'")

    def test_base_allows_safe_committed_change(self):
        base = self.baseline(agents=True)
        self.put("src/new.rs")
        self.git("add", "--", "src/new.rs")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic safe change")
        self.assert_passes(self.guard("--base", base))

    def test_forbidden_artifact_removal_is_allowed(self):
        self.put(".env.local")
        self.git("add", "--force", "--", ".env.local")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic forbidden baseline")
        (self.repo / ".env.local").unlink()
        self.git("add", "--update", "--", ".env.local")
        self.assert_passes(self.guard("--staged"))

    def test_non_repository_failure_is_closed(self):
        outside = self.root / "not-a-repository"
        outside.mkdir()
        self.assert_rejects(self.guard(cwd=outside), "Git rev-parse --show-toplevel failed")

    def test_invalid_base_failure_is_closed(self):
        self.baseline()
        self.assert_rejects(self.guard("--base", "missing-commit"), "Git rev-parse")

    def test_pre_commit_rejects_main(self):
        self.baseline()
        self.install_hooks()
        self.git("branch", "--move", "main")
        self.put("src/main.rs", "fn main() { /* synthetic change */ }\n")
        self.git("add", "--", "src/main.rs")
        result = self.run_command(["git", "commit", "--quiet", "-m", "Must fail"])
        self.assert_rejects(result, "direct commits on main")

    def test_pre_commit_checks_staged_policy_from_subdirectory(self):
        self.baseline()
        self.install_hooks()
        self.put(".env.local")
        self.git("add", "--force", "--", ".env.local")
        result = self.run_command(
            ["git", "commit", "--quiet", "-m", "Must fail"], cwd=self.repo / "src",
        )
        self.assert_rejects(result, "'.env.local'")

    def test_pre_commit_allows_safe_feature_commit(self):
        self.baseline()
        self.install_hooks()
        self.put("src/new.rs")
        self.git("add", "--", "src/new.rs")
        self.git("commit", "--quiet", "-m", "Safe feature commit")

    def test_relative_hooks_path_works_in_linked_worktree(self):
        self.baseline()
        self.install_hooks()
        self.git("add", "--", ".githooks", "scripts")
        self.git("commit", "--quiet", "--no-verify", "-m", "Synthetic hook installation")
        linked = self.root / "linked worktree"
        self.git("worktree", "add", "--quiet", "-b", "linked-feature", str(linked))
        self.put(".env.local", repo=linked)
        self.git("add", "--force", "--", ".env.local", cwd=linked)
        result = self.run_command(["git", "commit", "--quiet", "-m", "Must fail"], cwd=linked)
        self.assert_rejects(result, "'.env.local'")

    def test_pre_push_reads_all_updates_and_rejects_main_destination(self):
        self.install_hooks()
        oid = "1" * 40
        zero = "0" * 40
        updates = (
            f"refs/heads/feature {oid} refs/heads/feature {zero}\n"
            f"HEAD {oid} refs/heads/main {zero}\n"
            f"refs/heads/other {oid} refs/heads/other {zero}\n"
        )
        result = self.run_command([str(self.repo / ".githooks" / "pre-push")], input=updates)
        self.assert_rejects(result, "direct pushes to main")

    def test_pre_push_allows_feature_updates(self):
        self.install_hooks()
        updates = f"HEAD {'1' * 40} refs/heads/feature {'0' * 40}\n"
        self.assert_passes(self.run_command(
            [str(self.repo / ".githooks" / "pre-push")], input=updates,
        ))

    def test_pre_push_rejects_malformed_updates(self):
        self.install_hooks()
        result = self.run_command(
            [str(self.repo / ".githooks" / "pre-push")], input="malformed\n",
        )
        self.assert_rejects(result, "malformed pre-push ref update")

    def test_pre_push_rejects_unterminated_main_update(self):
        self.install_hooks()
        result = self.run_command(
            [str(self.repo / ".githooks" / "pre-push")],
            input=f"HEAD {'1' * 40} refs/heads/main {'0' * 40}",
        )
        self.assert_rejects(result, "direct pushes to main")

    def test_actual_push_head_to_main_is_rejected(self):
        self.baseline()
        self.install_hooks()
        remote = self.root / "remote.git"
        self.git("init", "--bare", "--quiet", str(remote))
        result = self.run_command(["git", "push", str(remote), "HEAD:main"])
        self.assert_rejects(result, "direct pushes to main")


if __name__ == "__main__":
    unittest.main()
