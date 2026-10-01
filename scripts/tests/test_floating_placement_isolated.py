"""FERESE_TEST_FLOATING=1 python3 scripts/tests/test_floating_placement_isolated.py

Real client mapping, remembered placement, and overlap on a disposable compositor.
Requires built debug ferese/feresectl, cc, wayland-scanner and a Wayland host.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("FERESE_TEST_FLOATING") == "1", "nested test is opt-in")
class FloatingPlacementTest(unittest.TestCase):
    def test_first_buffer_preserves_memory_and_new_windows_avoid_overlap(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="ferese-placement-test-") as temporary:
            root = Path(temporary)
            protocols = subprocess.check_output(["pkg-config", "--variable=pkgdatadir", "wayland-protocols"], text=True).strip()
            xml = str(Path(protocols) / "stable/xdg-shell/xdg-shell.xml")
            for kind, output in [("client-header", "xdg-shell-client-protocol.h"), ("private-code", "xdg-shell-protocol.c")]:
                subprocess.run(["wayland-scanner", kind, xml, str(root / output)], check=True)
            flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "wayland-client"], text=True).split()
            subprocess.run(["cc", "-Wall", "-Wextra", "-I", str(root), str(repo / "scripts/tests/fixtures/floating-size.c"),
                            str(root / "xdg-shell-protocol.c"), "-o", str(root / "client"), *flags], check=True)
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            config = root / "config/ferese"
            config.mkdir(parents=True)
            (config / "config.kdl").write_text('animations { enabled #false; }\n' + ''.join(
                f'window-rule app-id="ferese.test.placement.{app}" floating=#true\n' for app in ["remembered", "second", "third"]) +
                'window-rule app-id="ferese.test.placement.fullscreen" floating=#true width=240 height=160 fullscreen=#true\n' +
                'window-rule app-id="ferese.test.placement.maximized" floating=#true width=240 height=160\n')
            state = root / "state/ferese"
            state.mkdir(parents=True)
            memory = state / "floating.json"
            memory.write_text(json.dumps({"ferese.test.placement.remembered": {"output": "ferese-winit", "fractions": [.1, .1, .2, .2]}}))
            before = memory.read_bytes()
            display = Path(os.environ["WAYLAND_DISPLAY"])
            if not display.is_absolute():
                display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
            env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(root / "config"),
                       XDG_STATE_HOME=str(root / "state"), WAYLAND_DISPLAY=str(display))
            env.pop("WAYLAND_SOCKET", None)
            env.pop("FERESE_SHELL_CONTROL_SOCKET", None)
            children = []

            def launch(command, **kwargs):
                child = subprocess.Popen(command, env=env, start_new_session=True, **kwargs)
                children.append(child)
                return child

            def command(*args):
                return json.loads(subprocess.check_output([str(repo / "target/debug/feresectl"), *args], env=env, timeout=5))

            with (root / "compositor.log").open("w") as log:
                try:
                    bus = launch(["dbus-daemon", "--session", "--nofork", "--print-address=1"], stdout=subprocess.PIPE, text=True)
                    env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
                    compositor = launch([str(repo / "target/debug/ferese"), "--backend", "nested"], stdout=log, stderr=log)
                    deadline = time.monotonic() + 10
                    while not (runtime / "ferese/control.sock").is_socket():
                        if compositor.poll() is not None or time.monotonic() > deadline:
                            self.fail((root / "compositor.log").read_text())
                        time.sleep(.025)
                    socket = next(p for p in runtime.glob("wayland-*") if not p.name.endswith(".lock"))
                    env["WAYLAND_DISPLAY"] = str(socket)
                    for app in ["remembered", "second", "third"]:
                        launch([str(root / "client"), "ferese.test.placement." + app, "240", "160"], stdout=log, stderr=log)
                        deadline = time.monotonic() + 5
                        while True:
                            windows = command("get-windows")
                            if any(w["app_id"] == "ferese.test.placement." + app and w["width"] == 240 and w["height"] == 160 for w in windows):
                                break
                            if time.monotonic() > deadline:
                                self.fail((root / "compositor.log").read_text() + repr(windows))
                            time.sleep(.025)
                    remembered = next(w for w in windows if w["app_id"].endswith("remembered"))
                    output = next(o for o in command("get-outputs") if o["enabled"])
                    self.assertAlmostEqual(remembered["x"], output["x"] + .1 * output["width"], delta=1)
                    self.assertAlmostEqual(remembered["y"], output["y"] + .1 * output["height"], delta=1)
                    for i, a in enumerate(windows):
                        for b in windows[i+1:]:
                            width = max(0, min(a["x"]+a["width"], b["x"]+b["width"])-max(a["x"], b["x"]))
                            height = max(0, min(a["y"]+a["height"], b["y"]+b["height"])-max(a["y"], b["y"]))
                            self.assertEqual(width * height, 0, repr(windows))
                    self.assertEqual(memory.read_bytes(), before, "mapping must not save geometry every frame")
                    # The third window is focused. Its old floating rectangle
                    # must survive a trip through the tiled layout.
                    third = next(w for w in windows if w["app_id"].endswith("third"))
                    command("toggle-floating")
                    command("toggle-floating")
                    deadline = time.monotonic() + 5
                    expected = tuple(third[k] for k in ["x", "y", "width", "height"])
                    while True:
                        restored = next(w for w in command("get-windows") if w["app_id"].endswith("third"))
                        actual = tuple(restored[k] for k in ["x", "y", "width", "height"])
                        if actual == expected:
                            break
                        if time.monotonic() > deadline:
                            self.fail(f"floating rectangle changed after toggling: {expected} -> {actual}")
                        time.sleep(.025)
                    self.assertEqual(memory.read_bytes(), before, "nested previews must not persist drag memory")
                    for mode in ["fullscreen", "maximized"]:
                        app = "ferese.test.placement." + mode
                        if mode == "maximized":
                            # Tree mode exercises the full-work-area maximization
                            # path; scrolling mode uses column maximization.
                            command("toggle-layout")
                        launch([str(root / "client"), app, "240", "160", "configured"], stdout=log, stderr=log)
                        if mode == "maximized":
                            deadline = time.monotonic() + 5
                            while not any(w["app_id"] == app and w["capture_width"] == 240 for w in command("get-windows")):
                                if time.monotonic() > deadline:
                                    self.fail("maximized regression client did not map")
                                time.sleep(.025)
                            command("toggle-maximized")
                        deadline = time.monotonic() + 5
                        while True:
                            enlarged = next((w for w in command("get-windows") if w["app_id"] == app), None)
                            if enlarged and enlarged["capture_width"] > 240 and enlarged["capture_height"] > 160:
                                break
                            if time.monotonic() > deadline:
                                self.fail(f"{mode} client did not commit its enlarged buffer: {enlarged}")
                            time.sleep(.025)
                        command("toggle-" + mode)
                        deadline = time.monotonic() + 5
                        while True:
                            restored = next(w for w in command("get-windows") if w["app_id"] == app)
                            if (restored["width"], restored["height"], restored["capture_width"], restored["capture_height"]) == (240, 160, 240, 160):
                                break
                            if time.monotonic() > deadline:
                                self.fail(f"{mode} first buffer overwrote normal floating geometry: {restored}")
                            time.sleep(.025)
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
