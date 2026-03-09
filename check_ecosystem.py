#!/usr/bin/env python3
"""Ecosystem compatibility test runner for httprs.

Clones target repos, runs their pytest suites against real httpx (baseline)
and the httprs compat shim (experiment), then reports regressions.

Usage:
    # Quick smoke test (one repo, skip baseline)
    python check_ecosystem.py --repos openai anthropic --no-baseline --timeout 120

    # Full run
    python check_ecosystem.py --checkouts-dir /tmp/httprs-eco -v

    # With a pre-built wheel
    python check_ecosystem.py --httprs-wheel target/wheels/httprs-*.whl
"""

from __future__ import annotations

import argparse
import asyncio
import datetime
import os
import re
import shutil
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import NamedTuple

REPO_ROOT = Path(__file__).parent
ECOSYSTEM_CONFTEST = REPO_ROOT / "ecosystem_conftest.py"


class Repository(NamedTuple):
    org: str
    repo: str
    ref: str
    pytest_args: tuple[str, ...] = ()
    install_extras: str = ".[dev,test]"
    # Extra packages pip-installed after install_extras (e.g. test tooling not in extras).
    extra_pip: tuple[str, ...] = ()


REPOS: list[Repository] = [
    Repository(
        "openai",
        "openai-python",
        "main",
        pytest_args=(
            "tests/",
            # openai's pyproject.toml sets addopts = "-n auto" (pytest-xdist);
            # override it so pytest doesn't crash without xdist installed.
            "--override-ini=addopts=",
            # api_resources tests make real HTTP calls to api.openai.com and take
            # 1+ hours; they require a live API key so are not useful for compat testing.
            "--ignore=tests/api_resources",
            # Cap any individual test that hangs (e.g. stray real HTTP call).
            "--timeout=10",
        ),
        # .[datalib] only adds numpy/pandas; test infra is in rye dev-dependencies
        # which uv pip install can't reach, so install explicitly.
        install_extras=".[datalib]",
        extra_pip=(
            "respx",
            "pytest-asyncio",
            "anyio[trio]",
            "dirty-equals",
            "rich",
            "inline-snapshot",
            "pytest-timeout",
        ),
    ),
    Repository(
        "anthropics",
        "anthropic-sdk-python",
        "main",
        pytest_args=(
            "tests/",
            "--override-ini=addopts=",
            "--ignore=tests/api_resources",
            "--timeout=10",
        ),
        install_extras=".",
        extra_pip=(
            "pytest-asyncio",
            "dirty-equals",
            "inline-snapshot",
            "pytest-timeout",
            "http-snapshot[httpx]",
            "time-machine",
        ),
    ),
]

# Keys used on the CLI (short names) and the long repo name.
_REPO_BY_KEY: dict[str, Repository] = {
    "openai": REPOS[0],
    "anthropic": REPOS[1],
}


@dataclass
class TestOutcome:
    passed: int = 0
    failed: int = 0
    error: int = 0
    skipped: int = 0
    exit_code: int = 0
    failing_tests: list[str] = field(default_factory=list)
    failure_details: dict[str, str] = field(default_factory=dict)


@dataclass
class RepoDiff:
    repo: Repository
    baseline: TestOutcome
    experiment: TestOutcome
    regressions: list[str]
    pre_existing: list[str]


async def _run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout_secs: int | None = None,
    verbose: bool = False,
) -> tuple[int, str]:
    """Run *cmd*, return (returncode, combined stdout+stderr)."""
    if verbose:
        print(f"    $ {' '.join(str(c) for c in cmd)}", flush=True)
    proc = await asyncio.create_subprocess_exec(
        *[str(c) for c in cmd],
        cwd=cwd,
        env=env,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    try:
        stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout_secs)
        return proc.returncode, stdout.decode(errors="replace")
    except asyncio.TimeoutError:
        proc.kill()
        await proc.communicate()
        return -1, "(timeout)"


def _parse_pytest_output(output: str, exit_code: int) -> TestOutcome:
    """Parse pytest -v --tb=line output into a TestOutcome."""
    failing: list[str] = []
    details: dict[str, str] = {}

    for raw_line in output.splitlines():
        line = raw_line.strip()
        # "FAILED tests/foo.py::test_bar - SomeError: ..."
        if line.startswith("FAILED "):
            parts = line[7:].split(" - ", 1)
            node_id = parts[0].strip()
            failing.append(node_id)
            if len(parts) > 1 and parts[1]:
                details[node_id] = parts[1].strip()
        # "ERROR tests/foo.py::test_bar" (collection/setup errors)
        elif line.startswith("ERROR "):
            parts = line[6:].split(" - ", 1)
            node_id = parts[0].strip()
            if "::" in node_id or node_id.endswith(".py"):
                failing.append(node_id)
                if len(parts) > 1 and parts[1]:
                    details[node_id] = parts[1].strip()

    # Parse summary: "== 12 passed, 3 failed, 1 error, 5 skipped in 4.20s =="
    passed = failed = error = skipped = 0
    m = re.search(r"={2,}\s+(.+?)\s+in\s+[\d.]+s", output)
    if m:
        for part in m.group(1).split(", "):
            part = part.strip()
            nm = re.match(r"(\d+)\s+(\w+)", part)
            if nm:
                n, label = int(nm.group(1)), nm.group(2)
                if "passed" in label:
                    passed = n
                elif "failed" in label:
                    failed = n
                elif "error" in label:
                    error = n
                elif "skip" in label:
                    skipped = n

    return TestOutcome(
        passed=passed,
        failed=failed,
        error=error,
        skipped=skipped,
        exit_code=exit_code,
        failing_tests=failing,
        failure_details=details,
    )


async def _clone(
    repo: Repository,
    checkouts_dir: Path,
    clean: bool,
    verbose: bool,
) -> Path:
    dest = checkouts_dir / repo.repo
    if clean and dest.exists():
        shutil.rmtree(dest)
    if dest.exists():
        if verbose:
            print(f"  Reusing checkout: {dest}", flush=True)
        return dest
    if verbose:
        print(f"  Cloning {repo.org}/{repo.repo} @ {repo.ref}...", flush=True)
    rc, out = await _run(
        [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            repo.ref,
            f"https://github.com/{repo.org}/{repo.repo}.git",
            str(dest),
        ],
        verbose=verbose,
    )
    if rc != 0:
        raise RuntimeError(f"git clone failed:\n{out}")
    return dest


async def _setup_venv(
    checkout_dir: Path,
    repo: "Repository",
    wheel_path: Path,
    verbose: bool,
) -> Path:
    venv_dir = checkout_dir / ".venv-ecosystem"
    if venv_dir.exists():
        if verbose:
            print(f"  Reusing venv: {venv_dir}", flush=True)
        python_bin = venv_dir / "bin" / "python"
        if verbose:
            print("  Reinstalling httprs wheel...", flush=True)
        rc, out = await _run(
            [
                "uv",
                "pip",
                "install",
                "--python",
                str(python_bin),
                "--force-reinstall",
                str(wheel_path),
            ],
            cwd=checkout_dir,
            verbose=verbose,
        )
        if rc != 0:
            raise RuntimeError(f"uv pip install (wheel reinstall) failed:\n{out}")
        return venv_dir

    python_bin = venv_dir / "bin" / "python"

    # Try `uv sync` first — it installs [dependency-groups] (e.g. dev) that
    # `uv pip install` silently skips.  UV_PROJECT_ENVIRONMENT redirects the
    # venv from the default `.venv` to our `.venv-ecosystem`.
    if verbose:
        print("  Syncing dependencies (uv sync)...", flush=True)
    sync_env = {**os.environ, "UV_PROJECT_ENVIRONMENT": str(venv_dir)}
    rc, out = await _run(
        ["uv", "sync", "--python", "3.12", "--all-extras", "--inexact"],
        cwd=checkout_dir,
        env=sync_env,
        verbose=verbose,
    )
    if rc != 0:
        # Fall back to plain pip install for repos not managed by uv (e.g. rye).
        if verbose:
            print(
                f"  uv sync failed; falling back to pip install {repo.install_extras}...",
                flush=True,
            )
        rc, out = await _run(
            ["uv", "venv", "--python", "3.12", str(venv_dir)],
            cwd=checkout_dir,
            verbose=verbose,
        )
        if rc != 0:
            raise RuntimeError(f"uv venv failed:\n{out}")

        rc, out = await _run(
            ["uv", "pip", "install", "--python", str(python_bin), repo.install_extras],
            cwd=checkout_dir,
            verbose=verbose,
        )
        if rc != 0:
            raise RuntimeError(f"uv pip install (extras) failed:\n{out}")

    # Always ensure pytest is present (sync may omit it if not in default groups).
    rc, out = await _run(
        ["uv", "pip", "install", "--python", str(python_bin), "pytest"],
        cwd=checkout_dir,
        verbose=verbose,
    )
    if rc != 0:
        raise RuntimeError(f"uv pip install (pytest) failed:\n{out}")

    # Install any repo-specific test packages not reachable via sync/extras.
    if repo.extra_pip:
        if verbose:
            print(
                f"  Installing extra test deps: {' '.join(repo.extra_pip)}...",
                flush=True,
            )
        rc, out = await _run(
            ["uv", "pip", "install", "--python", str(python_bin), *repo.extra_pip],
            cwd=checkout_dir,
            verbose=verbose,
        )
        if rc != 0:
            raise RuntimeError(f"uv pip install (extra_pip) failed:\n{out}")

    if verbose:
        print("  Installing httprs wheel...", flush=True)
    rc, out = await _run(
        [
            "uv",
            "pip",
            "install",
            "--python",
            str(python_bin),
            "--force-reinstall",
            str(wheel_path),
        ],
        cwd=checkout_dir,
        verbose=verbose,
    )
    if rc != 0:
        raise RuntimeError(f"uv pip install (wheel) failed:\n{out}")

    return venv_dir


async def _run_pytest(
    checkout_dir: Path,
    venv_dir: Path,
    pytest_args: tuple[str, ...],
    *,
    inject_compat: bool,
    timeout_secs: int,
    verbose: bool,
    extra_env: dict[str, str] | None = None,
) -> TestOutcome:
    python_bin = venv_dir / "bin" / "python"
    cmd: list[str] = [
        str(python_bin),
        "-m",
        "pytest",
        "-v",
        "--tb=line",
        "--no-header",
        *pytest_args,
    ]
    env = os.environ.copy()
    if extra_env:
        env.update(extra_env)

    tmpdir: str | None = None
    try:
        if inject_compat:
            tmpdir = tempfile.mkdtemp(prefix="httprs-conftest-")
            tmp_path = Path(tmpdir)
            # Build-layer injection: publish a temporary "httpx" package on
            # PYTHONPATH so any early plugin import resolves to httprs compat
            # without runtime sys.modules rewrites.
            httpx_pkg = tmp_path / "httpx"
            httpx_pkg.mkdir(parents=True, exist_ok=True)
            shutil.copy(REPO_ROOT / "httpx_compat.py", httpx_pkg / "__init__.py")

            # Named _httprs_compat (not conftest.py) so pytest loads it via -p,
            # not via its own conftest.py discovery.
            shutil.copy(ECOSYSTEM_CONFTEST, tmp_path / "_httprs_compat.py")
            env["PYTHONPATH"] = tmpdir + os.pathsep + env.get("PYTHONPATH", "")
            env["HTTPRS_COMPAT_SHIM"] = str(httpx_pkg / "__init__.py")
            cmd += ["-p", "_httprs_compat"]

        rc, output = await _run(
            cmd,
            cwd=checkout_dir,
            env=env,
            timeout_secs=timeout_secs,
            verbose=verbose,
        )
    finally:
        if tmpdir:
            shutil.rmtree(tmpdir, ignore_errors=True)

    return _parse_pytest_output(output, rc)


def _compute_diff(
    repo: Repository,
    baseline: TestOutcome,
    experiment: TestOutcome,
) -> RepoDiff:
    b_fail = set(baseline.failing_tests)
    e_fail = set(experiment.failing_tests)
    return RepoDiff(
        repo=repo,
        baseline=baseline,
        experiment=experiment,
        regressions=sorted(e_fail - b_fail),
        pre_existing=sorted(b_fail & e_fail),
    )


async def _build_wheel(verbose: bool) -> Path:
    if verbose:
        print("Building httprs release wheel...", flush=True)
    rc, out = await _run(
        ["uvx", "maturin", "build"],
        cwd=REPO_ROOT,
        verbose=verbose,
    )
    if rc != 0:
        raise RuntimeError(f"maturin build failed:\n{out}")
    wheels = sorted(
        (REPO_ROOT / "target" / "wheels").glob("httprs-*.whl"),
        key=lambda p: p.stat().st_mtime,
    )
    if not wheels:
        raise RuntimeError("No wheel found in target/wheels/ after build.")
    wheel = wheels[-1]
    if verbose:
        print(f"  Wheel: {wheel.name}", flush=True)
    return wheel


async def _process_repo(
    repo: Repository,
    checkouts_dir: Path,
    wheel_path: Path,
    args: argparse.Namespace,
    semaphore: asyncio.Semaphore,
) -> RepoDiff | None:
    label = f"{repo.org}/{repo.repo}"
    async with semaphore:
        if args.verbose:
            print(f"\n[{label}] Starting...", flush=True)
        try:
            checkout_dir = await _clone(repo, checkouts_dir, args.clean, args.verbose)
            venv_dir = await _setup_venv(checkout_dir, repo, wheel_path, args.verbose)

            extra_env: dict[str, str] = {}
            if repo.repo == "openai-python":
                # Skip live API calls; tests expecting OPENAI_API_KEY still run
                # (they'll fail on bad key rather than missing key).
                extra_env["OPENAI_API_KEY"] = "fake"
            if repo.repo == "anthropic-sdk-python":
                extra_env["ANTHROPIC_API_KEY"] = "fake"

            if args.no_baseline:
                if args.verbose:
                    print(
                        f"[{label}] Running experiment (httprs compat)...", flush=True
                    )
                experiment = await _run_pytest(
                    checkout_dir,
                    venv_dir,
                    repo.pytest_args,
                    inject_compat=True,
                    timeout_secs=args.timeout,
                    verbose=args.verbose,
                    extra_env=extra_env,
                )
                _print_counts(label, "experiment", experiment, args.verbose)
                return RepoDiff(
                    repo=repo,
                    baseline=TestOutcome(),
                    experiment=experiment,
                    regressions=[],
                    pre_existing=[],
                )

            if args.verbose:
                print(f"[{label}] Running baseline (real httpx)...", flush=True)
            baseline = await _run_pytest(
                checkout_dir,
                venv_dir,
                repo.pytest_args,
                inject_compat=False,
                timeout_secs=args.timeout,
                verbose=args.verbose,
                extra_env=extra_env,
            )
            _print_counts(label, "baseline", baseline, args.verbose)

            if args.verbose:
                print(f"[{label}] Running experiment (httprs compat)...", flush=True)
            experiment = await _run_pytest(
                checkout_dir,
                venv_dir,
                repo.pytest_args,
                inject_compat=True,
                timeout_secs=args.timeout,
                verbose=args.verbose,
                extra_env=extra_env,
            )
            _print_counts(label, "experiment", experiment, args.verbose)

            diff = _compute_diff(repo, baseline, experiment)
            if diff.regressions and args.verbose:
                print(
                    f"[{label}] !! {len(diff.regressions)} regression(s) detected",
                    flush=True,
                )
            return diff

        except Exception as exc:
            print(f"[{label}] ERROR: {exc}", file=sys.stderr, flush=True)
            if args.verbose:
                import traceback

                traceback.print_exc()
            return None


def _print_counts(label: str, run: str, outcome: TestOutcome, verbose: bool) -> None:
    if not verbose:
        return
    total = outcome.passed + outcome.failed + outcome.error + outcome.skipped
    suffix = ""
    if total == 0 and outcome.exit_code != 0:
        suffix = f" (exit {outcome.exit_code} — pytest may have crashed)"
    elif total == 0:
        suffix = " (no tests collected)"
    print(
        f"[{label}] {run}: {outcome.passed} passed, {outcome.failed} failed, "
        f"{outcome.skipped} skipped, {outcome.error} error{suffix}",
        flush=True,
    )


def _format_regression_rate(regressions: int, baseline: TestOutcome) -> str:
    """Format regressions as count + percentage of baseline-passing tests."""
    if baseline.passed <= 0:
        return f"{regressions} (n/a)"
    pct = (regressions / baseline.passed) * 100.0
    return f"{regressions} ({pct:.1f}%)"


def _format_report(diffs: list[RepoDiff], args: argparse.Namespace) -> tuple[str, int]:
    lines: list[str] = []
    today = datetime.date.today().isoformat()
    try:
        import importlib.metadata

        version = importlib.metadata.version("httprs")
    except Exception:
        version = "unknown"

    if not args.verbose:
        if args.no_baseline:
            lines += [
                "| repo | passed | failed | error | skipped |",
                "|---|---|---|---|---|",
            ]
            for diff in diffs:
                label = f"{diff.repo.org}/{diff.repo.repo}"
                e = diff.experiment
                lines.append(
                    f"| {label} | {e.passed} | {e.failed} | {e.error} | {e.skipped} |"
                )
            return "\n".join(lines), 0

        lines += [
            "| repo | baseline package | baseline passed | compat passed | remaining regressions |",
            "|---|---|---|---|---|",
        ]
        for diff in diffs:
            label = f"{diff.repo.org}/{diff.repo.repo}"
            regressions = len(diff.regressions)
            lines.append(
                f"| {label} "
                f"| httpx "
                f"| {diff.baseline.passed} "
                f"| {diff.experiment.passed} "
                f"| {_format_regression_rate(regressions, diff.baseline)} |"
            )
        return "\n".join(lines), 0

    lines += [
        "## Ecosystem Compatibility Report",
        f"httprs v{version} | {today}",
        "",
    ]

    for diff in diffs:
        label = f"{diff.repo.org}/{diff.repo.repo}"
        lines.append(f"### {label} @ {diff.repo.ref}")

        if args.no_baseline:
            lines += [
                "| | httprs (compat) |",
                "|---|---|",
                f"| passed | {diff.experiment.passed} |",
                f"| failed | {diff.experiment.failed} |",
                f"| error | {diff.experiment.error} |",
                f"| skipped | {diff.experiment.skipped} |",
            ]
        else:
            lines += [
                "| | httpx (baseline) | httprs (compat) |",
                "|---|---|---|",
            ]
            for stat in ("passed", "failed", "error", "skipped"):
                b = getattr(diff.baseline, stat)
                e = getattr(diff.experiment, stat)
                lines.append(f"| {stat} | {b} | {e} |")
            lines.append(
                f"| remaining regressions | - | "
                f"{_format_regression_rate(len(diff.regressions), diff.baseline)} |"
            )

        lines.append("")

        if diff.regressions:
            n = len(diff.regressions)
            lines.append(f"#### Regressions ({n} test{'s' if n != 1 else ''})")
            for t in diff.regressions:
                detail = diff.experiment.failure_details.get(t)
                if detail:
                    lines.append(f"- {t} — {detail}")
                else:
                    lines.append(f"- {t}")
            lines.append("")

        if not args.no_baseline and diff.pre_existing:
            n = len(diff.pre_existing)
            lines.append(f"#### Pre-existing failures ({n} tests)")
            for t in diff.pre_existing:
                lines.append(f"- {t}")
            lines.append("")

    if len(diffs) > 1 and not args.no_baseline:
        lines += [
            "### Summary",
            "| repo | baseline passed | compat passed | remaining regressions |",
            "|---|---|---|---|",
        ]
        for diff in diffs:
            label = f"{diff.repo.org}/{diff.repo.repo}"
            lines.append(
                f"| {label} "
                f"| {diff.baseline.passed} "
                f"| {diff.experiment.passed} "
                f"| {_format_regression_rate(len(diff.regressions), diff.baseline)} |"
            )
        lines.append("")

    return "\n".join(lines), 0


async def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run ecosystem compatibility tests for httprs.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--checkouts-dir",
        default="/tmp/httprs-ecosystem",
        metavar="PATH",
        help="Directory for repo checkouts (default: /tmp/httprs-ecosystem)",
    )
    parser.add_argument(
        "--repos",
        nargs="+",
        choices=list(_REPO_BY_KEY),
        metavar="REPO",
        help="Repos to test (default: all). Choices: openai, anthropic",
    )
    parser.add_argument(
        "--httprs-wheel",
        metavar="PATH",
        help="Path to a pre-built httprs wheel (skips the build step)",
    )
    parser.add_argument(
        "--no-baseline",
        action="store_true",
        help="Skip the real-httpx baseline run; only report httprs pass/fail",
    )
    parser.add_argument(
        "--clean",
        action="store_true",
        help="Remove existing checkouts before cloning",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=600,
        metavar="SECONDS",
        help="Per-repo pytest timeout in seconds (default: 600)",
    )
    parser.add_argument(
        "--concurrency",
        type=int,
        default=2,
        metavar="N",
        help="Max repos processed concurrently (default: 2)",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Print all failing test IDs and subprocess commands",
    )
    args = parser.parse_args()

    checkouts_dir = Path(args.checkouts_dir)
    checkouts_dir.mkdir(parents=True, exist_ok=True)

    # Resolve repos.
    if args.repos:
        seen: set[str] = set()
        repos: list[Repository] = []
        for key in args.repos:
            r = _REPO_BY_KEY[key]
            if r.repo not in seen:
                seen.add(r.repo)
                repos.append(r)
    else:
        repos = list(REPOS)

    # Resolve wheel.
    if args.httprs_wheel:
        wheel_path = Path(args.httprs_wheel).resolve()
        if not wheel_path.exists():
            print(f"ERROR: wheel not found: {wheel_path}", file=sys.stderr)
            return 2
    else:
        try:
            wheel_path = await _build_wheel(args.verbose)
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2

    semaphore = asyncio.Semaphore(args.concurrency)
    tasks = [
        _process_repo(repo, checkouts_dir, wheel_path, args, semaphore)
        for repo in repos
    ]
    results = await asyncio.gather(*tasks)
    diffs = [r for r in results if r is not None]

    report, exit_code = _format_report(diffs, args)
    print(("\n" if args.verbose else "") + report)
    return exit_code


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
