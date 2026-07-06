#!/usr/bin/env python3
"""
workspace_lint.py — Audit and fix project directory structure.

Reads a workspace-lint.yaml config file and validates the current project
directory against declared canonical structure, forbidden patterns, and
file placement rules.

Usage:
    python3 workspace_lint.py [--root PATH] [--config FILE] [--fix] [--summary]
"""

import argparse
import fnmatch
import os
import re
import shutil
import sys
import textwrap
from pathlib import Path
from typing import Any, Optional

try:
    import yaml
except ImportError:
    print("ERROR: PyYAML required. Install: pip install PyYAML", file=sys.stderr)
    sys.exit(2)


CONFIG_NAMES = [
    "workspace-lint.yaml",
    "workspace-lint.yml",
    ".workspace-lint.yaml",
    ".workspace-lint.yml",
    "wlint.yaml",
    "wlint.yml",
]


def load_config(config_path: Optional[str], root: Path) -> dict:
    """Load the lint config, resolving the path."""
    if config_path:
        p = Path(config_path)
        if not p.exists():
            print(f"ERROR: config not found: {p}", file=sys.stderr)
            sys.exit(2)
    else:
        for name in CONFIG_NAMES:
            p = root / name
            if p.exists():
                break
        else:
            print(
                f"ERROR: no config found. Looked for: {', '.join(CONFIG_NAMES)}\n"
                f"Create one (see references/examples.md) or pass --config.",
                file=sys.stderr,
            )
            sys.exit(2)

    with open(p) as f:
        cfg = yaml.safe_load(f)

    if not isinstance(cfg, dict):
        print(f"ERROR: config is not a YAML dict: {p}", file=sys.stderr)
        sys.exit(2)

    return cfg


class Violation:
    def __init__(self, path: str, rule: str, message: str, severity: str, fixable: bool = False):
        self.path = path
        self.rule = rule
        self.message = message
        self.severity = severity  # error | warn | info
        self.fixable = fixable

    def __str__(self):
        return f"{self.path}:{self.rule}: {self.message} [{self.severity}]"

    def to_dict(self):
        return {
            "path": self.path,
            "rule": self.rule,
            "message": self.message,
            "severity": self.severity,
            "fixable": self.fixable,
        }


def _collect_files(root: Path, ignore_dirs: set) -> list[Path]:
    """Collect all files under root, skipping ignored directories."""
    files = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [
            d
            for d in dirnames
            if not d.startswith(".")
            and d not in ignore_dirs
            and d != "__pycache__"
            and d != "node_modules"
        ]
        for fname in filenames:
            files.append(Path(dirpath) / fname)
    return files


def _relpath(p: Path, root: Path) -> str:
    """Relative path string from root, forward slashes."""
    return str(p.relative_to(root)).replace("\\", "/")


def check_root_forbidden(files: list[Path], root: Path, rules: dict) -> list[Violation]:
    """Check that root directory doesn't contain forbidden files."""
    violations = []
    forbidden = rules.get("root", {}).get("forbidden_files", [])
    allowed = set(rules.get("root", {}).get("allowed_root_files", []))

    for f in files:
        if f.parent != root:
            continue
        rel = _relpath(f, root)
        if rel in allowed:
            continue
        for pattern in forbidden:
            if fnmatch.fnmatch(rel, pattern):
                violations.append(
                    Violation(
                        path=rel,
                        rule="root.forbidden_files",
                        message=f"File '{rel}' matches forbidden pattern '{pattern}' in project root",
                        severity="error",
                        fixable=False,  # We don't know where to move it without a specific rule
                    )
                )
    return violations


def check_dir_naming(root: Path, rules: dict, ignore_dirs: set | None = None) -> list[Violation]:
    """Check directory naming rules: no leading/trailing whitespace, no duplicates.

    The `ignore_dirs` set is honored via os.walk's top-down pruning: when we
    encounter a directory whose name is in `ignore_dirs`, we remove it from
    `dirnames` in place so os.walk does not descend into it. This prevents
    false-positive violations on ignored trees like `node_modules`, `target`,
    and `.venv` (without this, every nested `node_modules` inside npm
    packages would trip `directories.forbidden_patterns`).
    """
    violations = []
    forbidden_patterns = rules.get("directories", {}).get("forbidden_patterns", [])
    ignore = ignore_dirs or set()

    seen_dirs = {}
    for dirpath, dirnames, filenames in os.walk(root):
        # Prune ignored directories in place so os.walk does not descend.
        dirnames[:] = [d for d in dirnames if d not in ignore and not d.startswith(".")]
        for d in dirnames:
            full = Path(dirpath) / d
            rel = _relpath(full, root)

            for pattern in forbidden_patterns:
                try:
                    if re.search(pattern, d):
                        violations.append(
                            Violation(
                                path=rel,
                                rule="directories.forbidden_patterns",
                                message=f"Directory '{d}' matches forbidden pattern: {pattern}",
                                severity="error",
                                fixable=False,
                            )
                        )
                except re.error:
                    pass  # Skip invalid regex

            # Check for whitespace-variant duplicates (e.g., "1. PHANTOM" vs "1.PHANTOM")
            stripped = d.strip()
            if stripped != d:
                violations.append(
                    Violation(
                        path=rel,
                        rule="directories.whitespace_name",
                        message=f"Directory has leading/trailing whitespace: '{d}'",
                        severity="error",
                        fixable=False,
                    )
                )

            parent_key = str(full.parent)
            if parent_key not in seen_dirs:
                seen_dirs[parent_key] = {}
            norm_name = stripped.lower().replace(" ", "")
            if norm_name in seen_dirs[parent_key]:
                dup_rel = seen_dirs[parent_key][norm_name]
                violations.append(
                    Violation(
                        path=rel,
                        rule="directories.duplicate",
                        message=f"Possible duplicate of '{dup_rel}' (normalized: {norm_name})",
                        severity="warn",
                        fixable=False,
                    )
                )
            seen_dirs[parent_key][norm_name] = rel

    return violations


def check_empty_dirs(root: Path, rules: dict) -> list[Violation]:
    """Warn on empty directories (unless .gitkeep present)."""
    violations = []
    canonical = rules.get("structure", {}).get("canonical", [])

    for entry in canonical:
        path = root / entry.get("path", "")
        if not path.exists():
            violations.append(
                Violation(
                    path=entry["path"],
                    rule="structure.missing_canonical",
                    message=f"Canonical directory '{entry['path']}' does not exist",
                    severity="error",
                    fixable=False,
                )
            )
            continue
        if path.is_dir():
            contents = list(path.iterdir())
            has_gitkeep = any(c.name == ".gitkeep" for c in contents)
            if not contents and not has_gitkeep:
                violations.append(
                    Violation(
                        path=entry["path"],
                        rule="structure.empty_canonical",
                        message=f"Canonical directory '{entry['path']}' is empty (add .gitkeep or remove from config)",
                        severity="warn",
                        fixable=False,
                    )
                )
    return violations


def check_file_placement(files: list[Path], root: Path, rules: dict) -> list[Violation]:
    """Check that files are in their preferred directories."""
    violations = []
    file_rules = rules.get("files", {})

    for f in files:
        rel = _relpath(f, root)
        suffix = f.suffix.lower() if f.suffix else ""
        pattern = f"*{suffix}" if suffix else ""

        for rule_pattern, rule_config in file_rules.items():
            if not fnmatch.fnmatch(rel, rule_pattern):
                continue

            preferred = rule_config.get("preferred_dir")
            if preferred:
                preferred_path = str(Path(preferred).as_posix())
                if not rel.startswith(preferred_path + "/") and rel != preferred_path:
                    violations.append(
                        Violation(
                            path=rel,
                            rule="files.preferred_dir",
                            message=f"'{rel}' should be in '{preferred}/' (pattern: {rule_pattern})",
                            severity="warn",
                            fixable=False,  # We won't auto-move without explicit direction
                        )
                    )

            max_kb = rule_config.get("max_size_kb")
            if max_kb:
                try:
                    size_kb = f.stat().st_size / 1024
                    if size_kb > max_kb:
                        violations.append(
                            Violation(
                                path=rel,
                                rule="files.max_size",
                                message=f"'{rel}' is {size_kb:.0f}KB (max: {max_kb}KB)",
                                severity="warn",
                                fixable=False,
                            )
                        )
                except OSError:
                    pass
    return violations


def check_build_artifacts(root: Path, rules: dict, ignore_dirs: set | None = None) -> list[Violation]:
    """Check for __pycache__ and other build artifacts that should not exist.

    Honors `ignore_dirs` so we do not flag `node_modules/dist` inside an
    already-ignored `node_modules` tree, or `target/build` inside the
    already-ignored `target` tree. Without this, every npm package's local
    `dist/` would trip the rule, even though the parent `node_modules` is
    already gitignored and irrelevant to the project's own structure.
    """
    violations = []
    artifact_dirs = ["__pycache__", "node_modules", ".pytest_cache", ".mypy_cache", "dist", "build"]
    ignore = ignore_dirs or set()

    for dirpath, dirnames, _ in os.walk(root):
        # Prune ignored dirs in place so we don't descend into node_modules/target/etc.
        dirnames[:] = [d for d in dirnames if d not in ignore]
        for d in dirnames:
            if d in artifact_dirs:
                full = Path(dirpath) / d
                rel = _relpath(full, root)
                violations.append(
                    Violation(
                        path=rel,
                        rule="artifacts.detected",
                        message=f"Build artifact directory '{d}' should not be committed",
                        severity="error",
                        fixable=True,
                    )
                )
    return violations


def apply_fixes(root: Path, violations: list[Violation], dry_run: bool = False) -> int:
    """Apply auto-fixable violations. Returns count of fixes applied."""
    fixed = 0
    for v in violations:
        if not v.fixable:
            continue

        target = root / v.path
        if not target.exists():
            continue

        if v.rule == "artifacts.detected":
            if dry_run:
                print(f"  [dry-run] Would remove: {v.path}")
            else:
                shutil.rmtree(target)
                print(f"  [fixed] Removed: {v.path}")
            fixed += 1

    return fixed


def print_summary(violations: list[Violation]):
    errors = [v for v in violations if v.severity == "error"]
    warns = [v for v in violations if v.severity == "warn"]
    infos = [v for v in violations if v.severity == "info"]
    fixable = [v for v in violations if v.fixable]

    print()
    print("─" * 60)
    print(f"  Workspace Lint Summary")
    print("─" * 60)
    print(f"  Errors:   {len(errors)}")
    print(f"  Warnings: {len(warns)}")
    print(f"  Info:     {len(infos)}")
    print(f"  Fixable:  {len(fixable)}")
    print("─" * 60)
    print()


def main():
    parser = argparse.ArgumentParser(
        description="Lint project directory structure against workspace-lint.yaml"
    )
    parser.add_argument("--root", default=".", help="Project root (default: cwd)")
    parser.add_argument("--config", default=None, help="Config file path")
    parser.add_argument("--fix", action="store_true", help="Auto-fix safe violations")
    parser.add_argument("--summary", action="store_true", help="Show only summary")
    parser.add_argument("--json", action="store_true", help="Output violations as JSON")
    args = parser.parse_args()

    root = Path(args.root).resolve()
    if not root.is_dir():
        print(f"ERROR: root is not a directory: {root}", file=sys.stderr)
        sys.exit(2)

    config = load_config(args.config, root)
    ignore_dirs = set(config.get("ignore_dirs", []))

    files = _collect_files(root, ignore_dirs)
    violations = []
    violations.extend(check_dir_naming(root, config, ignore_dirs))
    violations.extend(check_build_artifacts(root, config, ignore_dirs))
    violations.extend(check_root_forbidden(files, root, config))
    violations.extend(check_empty_dirs(root, config))
    violations.extend(check_file_placement(files, root, config))

    if args.fix and violations:
        fixes = apply_fixes(root, violations)
        if fixes:
            print(f"\nApplied {fixes} fixes.")
            files = _collect_files(root, ignore_dirs)
            violations = []
            violations.extend(check_dir_naming(root, config, ignore_dirs))
            violations.extend(check_build_artifacts(root, config, ignore_dirs))
            violations.extend(check_root_forbidden(files, root, config))
            violations.extend(check_empty_dirs(root, config))
            violations.extend(check_file_placement(files, root, config))

    if args.json:
        import json
        print(json.dumps([v.to_dict() for v in violations], indent=2))
    elif not args.summary:
        for v in violations:
            print(f"  {v}")

    print_summary(violations)

    if any(v.severity == "error" for v in violations):
        sys.exit(1)
    sys.exit(0)


if __name__ == "__main__":
    main()
