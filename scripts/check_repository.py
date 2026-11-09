#!/usr/bin/env python3
"""Check Git path metadata only; never inspect tracked file contents.

All modes reject forbidden paths in the index. --staged also protects AGENTS.md
in the staged diff; --base COMMIT protects it in the committed COMMIT..HEAD diff.
Deleting a forbidden artifact is allowed, but deleting AGENTS.md is not.
"""

import argparse
import os
from pathlib import PurePosixPath
import subprocess
import sys


class PolicyError(Exception):
    """A policy violation or an inability to establish repository policy."""


def git(*args, cwd=None):
    try:
        result = subprocess.run(
            ["git", *args], cwd=cwd, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=False,
        )
    except OSError as error:
        raise PolicyError(f"cannot execute Git: {error}") from error
    if result.returncode:
        detail = result.stderr.decode("utf-8", errors="backslashreplace").strip()
        raise PolicyError(f"Git {' '.join(args)} failed: {detail or result.returncode}")
    return result.stdout


def git_paths(*args, cwd):
    data = git(*args, cwd=cwd)
    if data and not data.endswith(b"\0"):
        raise PolicyError("Git returned an unterminated path list")
    return {os.fsdecode(path) for path in data.split(b"\0") if path}


def forbidden_reason(path):
    parts = PurePosixPath(path).parts
    for part in parts:
        if part == ".env" or part.startswith(".env."):
            return "environment files (including templates) must not be tracked"
        if part in {".lake", "target", "runs", ".worktrees", "__pycache__"}:
            return f"local/generated {part}/ paths must not be tracked"
    if PurePosixPath(path).suffix in {".pyc", ".pyo", ".pyd"}:
        return "generated Python bytecode/extensions must not be tracked"
    for parent, child in zip(parts, parts[1:]):
        if parent == "docs" and child in {"stray", "outdated"}:
            return f"local docs/{child}/ paths must not be tracked"
    return None


def check(staged=False, base=None):
    # Remove only Git's terminating newline, preserving whitespace in root paths.
    root_output = git("rev-parse", "--show-toplevel")
    if not root_output.endswith(b"\n"):
        raise PolicyError("Git did not return a repository worktree root")
    root = os.fsdecode(root_output[:-1])
    tracked = git_paths("ls-files", "--cached", "--full-name", "-z", "--", cwd=root)
    changed = set()
    if staged:
        changed = git_paths(
            "diff", "--cached", "--name-only", "--no-renames",
            "--no-ext-diff", "--no-textconv", "-z", "--", cwd=root,
        )
    elif base is not None:
        commit = git(
            "rev-parse", "--verify", "--end-of-options", f"{base}^{{commit}}", cwd=root,
        ).decode("ascii").strip()
        changed = git_paths(
            "diff", "--name-only", "--no-renames", "--no-ext-diff",
            "--no-textconv", "-z", commit, "HEAD", "--", cwd=root,
        )

    violations = []
    for path in sorted(tracked):
        reason = forbidden_reason(path)
        if reason:
            violations.append(f"{ascii(path)}: {reason}")
    for path in sorted(changed):
        if PurePosixPath(path).name == "AGENTS.md":
            violations.append(f"{ascii(path)}: AGENTS.md must not be modified, renamed, or deleted")
    if violations:
        raise PolicyError("\n".join(violations))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--staged", action="store_true", help="also protect staged AGENTS.md changes")
    mode.add_argument("--base", metavar="COMMIT", help="also protect AGENTS.md changes from COMMIT to HEAD")
    args = parser.parse_args()
    try:
        check(staged=args.staged, base=args.base)
    except PolicyError as error:
        print(f"Repository policy failed: {error}", file=sys.stderr)
        return 1
    print("Repository policy passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
