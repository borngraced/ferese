"""Workspace history against a temporary nested compositor.

FERESE_TEST_WORKSPACE_HISTORY=1 python3 scripts/tests/test_workspace_history_isolated.py
Requires target/debug/ferese, feresectl, dbus-daemon, and a Wayland host.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("FERESE_TEST_WORKSPACE_HISTORY") == "1", "nested workspace test is opt-in")
class WorkspaceHistoryTest(unittest.TestCase):
    def test_empty_workspace_history_and_explicit_ipc_selection(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="ferese-workspace-test-") as temporary:
            root = Path(temporary)
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            config = root / "config/ferese"
            config.mkdir(parents=True)
            source = config / "config.kdl"
            source.write_text("animations { enabled #false; }\n")
            display = Path(os.environ["WAYLAND_DISPLAY"])
            if not display.is_absolute():
                display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
            env = dict(os.environ, WAYLAND_DISPLAY=str(display), XDG_RUNTIME_DIR=str(runtime),
                       XDG_CONFIG_HOME=str(root / "config"))
            env.pop("WAYLAND_SOCKET", None)
            env.pop("FERESE_SHELL_CONTROL_SOCKET", None)
            children = []

            def launch(command, **kwargs):
                child = subprocess.Popen(command, env=env, start_new_session=True, **kwargs)
                children.append(child)
                return child

            def command(*args):
                return json.loads(subprocess.check_output(
                    [str(repo / "target/debug/feresectl"), *args], env=env, timeout=5
                ))

            def active():
                return next(workspace["name"] for workspace in command("get-workspaces") if workspace["active"])

            with (root / "compositor.log").open("w") as log:
                try:
                    bus = launch(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                                 stdout=subprocess.PIPE, text=True)
                    env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
                    compositor = launch([str(repo / "target/debug/ferese"), "--backend", "nested"],
                                        stdout=log, stderr=log)
                    deadline = time.monotonic() + 10
                    while not (runtime / "ferese/control.sock").is_socket():
                        if compositor.poll() is not None or time.monotonic() >= deadline:
                            self.fail((root / "compositor.log").read_text())
                        time.sleep(.025)
                    self.assertEqual(active(), "1")
                    command("workspace-back-and-forth")
                    self.assertEqual(active(), "1", "no history is a no-op")
                    command("workspace", "2")
                    self.assertEqual(active(), "2")
                    self.assertEqual({w["name"] for w in command("get-workspaces")}, {"1", "2"})
                    for expected in ["1", "2", "1", "2"]:
                        command("workspace-back-and-forth")
                        self.assertEqual(active(), expected)
                    command("workspace", "3")
                    self.assertEqual({w["name"] for w in command("get-workspaces")}, {"2", "3"},
                                     "old empty history is pruned")
                    replacement = source.with_suffix(".new")
                    replacement.write_text("animations { enabled #false; }\nworkspaces { auto-back-and-forth #true; }\n")
                    replacement.replace(source)
                    command("reload-config")
                    command("workspace", "3")
                    self.assertEqual(active(), "3", "literal IPC selection must not auto-toggle")
                    command("workspace-back-and-forth")
                    self.assertEqual(active(), "2", "reload must preserve history")
                    command("workspace-back-and-forth")
                    self.assertEqual(active(), "3")
                finally:
                    for child in reversed(children):
                        if child.poll() is None:
                            os.killpg(child.pid, signal.SIGTERM)
                            try:
                                child.wait(timeout=3)
                            except subprocess.TimeoutExpired:
                                os.killpg(child.pid, signal.SIGKILL)
                                child.wait(timeout=3)
                        if child.stdout is not None:
                            child.stdout.close()


if __name__ == "__main__":
    unittest.main()
