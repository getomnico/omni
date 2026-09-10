"""Subprocess tests for the Salesforce MCP launcher.

These run ``bin/omni-salesforce-mcp`` with controlled stub ``sf``, ``node``,
``python3`` and ``sf-mcp-server`` executables so the bootstrap, credential
scrubbing, and cleanup behavior can be verified without touching Salesforce or
the real CLI state. Secrets are never echoed: only the names of leaked
variables and whether credential material appeared in argv are reported.
"""

from __future__ import annotations

import os
import signal
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

import pytest

LAUNCHER = Path(__file__).resolve().parents[1] / "bin" / "omni-salesforce-mcp"

_SECRET_ENV_KEYS = (
    "SF_CLIENT_ID",
    "SF_PRIVATE_KEY",
    "SF_USERNAME",
    "SF_LOGIN_URL",
    "SF_ACCESS_TOKEN",
    "SF_INSTANCE_URL",
    "SF_ORG_ID",
)


@dataclass
class LauncherHarness:
    env: dict[str, str]
    state_root: Path
    out_dir: Path
    marker: Path


def _write_stub(path: Path, body: str) -> None:
    path.write_text("#!/bin/sh\nset -eu\n" + body)
    path.chmod(0o755)


@pytest.fixture
def launcher(tmp_path: Path) -> LauncherHarness:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    state_root = tmp_path / "state"
    state_root.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    marker = tmp_path / "auth-status"

    _write_stub(
        bin_dir / "sf",
        'printf "%s\\n" "$*" >> "$STUB_OUT/sf_args"\n'
        'if [ -n "${STUB_SF_FAIL:-}" ]; then exit 1; fi\n'
        "exit 0\n",
    )
    _write_stub(
        bin_dir / "node",
        "cat >/dev/null\n"
        'if [ -n "${STUB_NODE_FAIL:-}" ]; then exit 1; fi\n'
        'printf "OMNI_ORG_USERNAME=stub-user@example.com\\n"\n',
    )
    _write_stub(
        bin_dir / "python3",
        'if [ -n "${STUB_USERINFO_FAIL:-}" ]; then exit 1; fi\n'
        'printf "%s\\n" "${STUB_USERINFO_ORG_ID:-00D000000000001}"\n',
    )
    _write_stub(
        bin_dir / "sf-mcp-server",
        '{\n  printf "ARGV"\n  for arg in "$@"; do printf " %s" "$arg"; done\n'
        '  printf "\\n"\n} > "$STUB_OUT/argv"\n'
        'env > "$STUB_OUT/env"\n'
        'if [ -n "${STUB_MCP_SLEEP:-}" ]; then sleep "$STUB_MCP_SLEEP"; fi\n'
        'exit "${STUB_MCP_STATUS:-0}"\n',
    )

    env = {
        "PATH": f"{bin_dir}:{os.environ['PATH']}",
        "STUB_OUT": str(out_dir),
        "OMNI_SALESFORCE_STATE_ROOT": str(state_root),
        "OMNI_SALESFORCE_SOURCE_ID": "source-1",
        "OMNI_MCP_AUTH_STATUS_FILE": str(marker),
    }
    return LauncherHarness(env=env, state_root=state_root, out_dir=out_dir, marker=marker)


def _run(harness: LauncherHarness, extra_env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    env = {**harness.env, **extra_env}
    return subprocess.run(
        [str(LAUNCHER)],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )


def _read_env(harness: LauncherHarness) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in (harness.out_dir / "env").read_text().splitlines():
        key, separator, value = line.partition("=")
        if separator:
            env[key] = value
    return env


def _read_argv(harness: LauncherHarness) -> list[str]:
    line = (harness.out_dir / "argv").read_text().strip()
    assert line.startswith("ARGV")
    return line[len("ARGV") :].split()


def _assert_no_secret_env(env: dict[str, str]) -> None:
    leaked = [key for key in _SECRET_ENV_KEYS if key in env]
    if leaked:
        raise AssertionError("credential env leaked to MCP child: " + ", ".join(leaked))


def _assert_no_secret_argv(argv: list[str], secrets: list[str]) -> None:
    leaked = [secret for secret in secrets if any(secret in part for part in argv)]
    if leaked:
        raise AssertionError("credential material leaked into MCP child argv")


def _assert_state_clean(harness: LauncherHarness) -> None:
    assert list(harness.state_root.iterdir()) == []


def _assert_marker(harness: LauncherHarness, expected: str | None) -> None:
    if expected is None:
        assert not harness.marker.exists()
    else:
        assert harness.marker.read_text().strip() == expected


def test_jwt_flow_scrubs_credentials_and_cleans_state(launcher: LauncherHarness) -> None:
    result = _run(
        launcher,
        {
            "OMNI_SALESFORCE_AUTH_MODE": "jwt",
            "SF_CLIENT_ID": "3MVG9-consumer-key",
            "SF_PRIVATE_KEY": "PRIVATE-KEY-MATERIAL",
            "SF_USERNAME": "user@example.com",
            "SF_LOGIN_URL": "https://login.salesforce.com",
        },
    )
    assert result.returncode == 0, result.stderr
    env = _read_env(launcher)
    _assert_no_secret_env(env)
    argv = _read_argv(launcher)
    # The username is the org selector passed to the MCP server by design;
    # only key material must never appear in argv.
    _assert_no_secret_argv(
        argv,
        ["3MVG9-consumer-key", "PRIVATE-KEY-MATERIAL"],
    )
    assert argv == ["--orgs", "user@example.com", "--toolsets", "data", "--no-telemetry"]
    _assert_state_clean(launcher)
    _assert_marker(launcher, None)


def test_access_token_flow_scrubs_credentials_and_cleans_state(
    launcher: LauncherHarness,
) -> None:
    result = _run(
        launcher,
        {
            "OMNI_SALESFORCE_AUTH_MODE": "access_token",
            "SF_ACCESS_TOKEN": "ACCESS-TOKEN-MATERIAL",
            "SF_INSTANCE_URL": "https://acme.my.salesforce.com",
            "SF_LOGIN_URL": "https://login.salesforce.com",
        },
    )
    assert result.returncode == 0, result.stderr
    env = _read_env(launcher)
    _assert_no_secret_env(env)
    argv = _read_argv(launcher)
    _assert_no_secret_argv(argv, ["ACCESS-TOKEN-MATERIAL"])
    assert argv == [
        "--orgs",
        "stub-user@example.com",
        "--toolsets",
        "data",
        "--no-telemetry",
    ]
    _assert_state_clean(launcher)
    _assert_marker(launcher, None)


def test_jwt_login_failure_preserves_needs_user_auth(launcher: LauncherHarness) -> None:
    result = _run(
        launcher,
        {
            "OMNI_SALESFORCE_AUTH_MODE": "jwt",
            "SF_CLIENT_ID": "3MVG9-consumer-key",
            "SF_PRIVATE_KEY": "PRIVATE-KEY-MATERIAL",
            "SF_USERNAME": "user@example.com",
            "SF_LOGIN_URL": "https://login.salesforce.com",
            "STUB_SF_FAIL": "1",
        },
    )
    assert result.returncode == 78
    assert not (launcher.out_dir / "argv").exists()
    _assert_state_clean(launcher)
    _assert_marker(launcher, "needs_user_auth")


def test_userinfo_failure_preserves_needs_user_auth(launcher: LauncherHarness) -> None:
    result = _run(
        launcher,
        {
            "OMNI_SALESFORCE_AUTH_MODE": "access_token",
            "SF_ACCESS_TOKEN": "ACCESS-TOKEN-MATERIAL",
            "SF_INSTANCE_URL": "https://acme.my.salesforce.com",
            "SF_LOGIN_URL": "https://login.salesforce.com",
            "STUB_USERINFO_FAIL": "1",
        },
    )
    assert result.returncode == 78
    _assert_state_clean(launcher)
    _assert_marker(launcher, "needs_user_auth")


def test_org_mismatch_preserves_needs_user_auth(launcher: LauncherHarness) -> None:
    result = _run(
        launcher,
        {
            "OMNI_SALESFORCE_AUTH_MODE": "access_token",
            "SF_ACCESS_TOKEN": "00Dmismatch!ACCESS-TOKEN-MATERIAL",
            "SF_INSTANCE_URL": "https://acme.my.salesforce.com",
            "SF_LOGIN_URL": "https://login.salesforce.com",
            "STUB_USERINFO_ORG_ID": "00Dactual00000001",
        },
    )
    assert result.returncode == 78
    _assert_state_clean(launcher)
    _assert_marker(launcher, "needs_user_auth")


def test_cli_bootstrap_failure_preserves_needs_user_auth(launcher: LauncherHarness) -> None:
    result = _run(
        launcher,
        {
            "OMNI_SALESFORCE_AUTH_MODE": "access_token",
            "SF_ACCESS_TOKEN": "ACCESS-TOKEN-MATERIAL",
            "SF_INSTANCE_URL": "https://acme.my.salesforce.com",
            "SF_LOGIN_URL": "https://login.salesforce.com",
            "STUB_NODE_FAIL": "1",
        },
    )
    assert result.returncode == 78
    _assert_state_clean(launcher)
    _assert_marker(launcher, "needs_user_auth")


def test_signal_cleanup_kills_child_and_removes_state(launcher: LauncherHarness) -> None:
    env = {
        **launcher.env,
        "OMNI_SALESFORCE_AUTH_MODE": "jwt",
        "SF_CLIENT_ID": "3MVG9-consumer-key",
        "SF_PRIVATE_KEY": "PRIVATE-KEY-MATERIAL",
        "SF_USERNAME": "user@example.com",
        "SF_LOGIN_URL": "https://login.salesforce.com",
        "STUB_MCP_SLEEP": "30",
    }
    process = subprocess.Popen(
        [str(LAUNCHER)],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and not (launcher.out_dir / "argv").exists():
        time.sleep(0.05)
    assert (launcher.out_dir / "argv").exists(), "MCP child never started"
    process.send_signal(signal.SIGTERM)
    assert process.wait(timeout=15) == 143
    _assert_state_clean(launcher)
    _assert_marker(launcher, None)
