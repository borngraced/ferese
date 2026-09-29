"""Verify isolated window capture without exposing host desktop content.

FERESE_TEST_WINDOW_CAPTURE=1 python3 scripts/tests/test_window_capture_isolated.py
Requires built debug binaries, a Wayland host, cc, wayland-scanner and Pillow.
"""
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest

REPO = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.environ.get("FERESE_TEST_WINDOW_CAPTURE") == "1", "requires a Wayland host")
class WindowCapture(unittest.TestCase):
    def test_occluding_window_does_not_appear_and_rows_keep_orientation(self):
        from PIL import Image
        processes = []
        with tempfile.TemporaryDirectory(prefix="ferese-window-capture-") as directory:
            root = Path(directory)
            protocols = subprocess.check_output(["pkg-config", "--variable=pkgdatadir", "wayland-protocols"], text=True).strip()
            xml = str(Path(protocols) / "stable/xdg-shell/xdg-shell.xml")
            for kind, output in [("client-header", "xdg-shell-client-protocol.h"), ("private-code", "xdg-shell-protocol.c")]:
                subprocess.run(["wayland-scanner", kind, xml, str(root / output)], check=True)
            flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "wayland-client"], text=True).split()
            subprocess.run(["cc", "-Wall", "-Wextra", "-I", str(root), str(REPO / "scripts/tests/fixtures/window-capture.c"),
                            str(root / "xdg-shell-protocol.c"), "-o", str(root / "client"), *flags], check=True)
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            config = root / "config/ferese/config.kdl"
            config.parent.mkdir(parents=True)
            config.write_text('window-rule app-id="ferese.test.window-capture" floating=#true\n')
            display = Path(os.environ["WAYLAND_DISPLAY"])
            if not display.is_absolute():
                display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
            env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(root / "config"), WAYLAND_DISPLAY=str(display), FERESE_ENABLE_SCREENCOPY="1")
            env.pop("WAYLAND_SOCKET", None)
            env.pop("FERESE_SHELL_CONTROL_SOCKET", None)
            log = (root / "compositor.log").open("w")
            try:
                compositor = subprocess.Popen([str(REPO / "target/debug/ferese"), "--backend=nested"], env=env, stdout=log, stderr=log, start_new_session=True)
                processes.append(compositor)
                deadline = time.monotonic() + 10
                while not (runtime / "ferese/control.sock").exists():
                    if compositor.poll() is not None or time.monotonic() > deadline:
                        self.fail((root / "compositor.log").read_text())
                    time.sleep(0.02)
                sockets = list(runtime.glob("wayland-*"))
                env["WAYLAND_DISPLAY"] = str(next(path for path in sockets if not path.name.endswith(".lock")))

                def call(*args):
                    return subprocess.check_output([str(REPO / "target/debug/feresectl"), *args], env=env, timeout=10)

                def windows(count):
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        result = json.loads(call("get-windows"))
                        if len(result) == count and all(window["mapped"] and window["width"] == 640 for window in result):
                            return result
                        time.sleep(0.02)
                    self.fail((root / "compositor.log").read_text())

                processes.append(subprocess.Popen([str(root / "client")], env=env, stdout=log, stderr=log, start_new_session=True))
                target = windows(1)[0]
                processes.append(subprocess.Popen([str(root / "client"), "cover"], env=env, stdout=log, stderr=log, start_new_session=True))
                current = windows(2)
                cover = next(window for window in current if window["title"] == "Occluding window")
                self.assertEqual((target["x"], target["y"], target["width"], target["height"]),
                                 (cover["x"], cover["y"], cover["width"], cover["height"]))
                scale = json.loads(call("get-outputs"))[0]["scale"]
                with Image.open(io.BytesIO(call("screenshot-window", str(target["id"])))) as image:
                    self.assertEqual(image.size, (round(640 * scale), round(480 * scale)))
                    self.assertEqual(image.convert("RGB").getpixel((image.width // 2, image.height // 4)), (255, 0, 0))
                    self.assertEqual(image.convert("RGB").getpixel((image.width // 2, image.height * 3 // 4)), (0, 0, 255))
                    self.assertEqual(image.convert("RGBA").getpixel((round(40 * scale), round(80 * scale))), (255, 0, 0, 128))
                with Image.open(io.BytesIO(call("screenshot-window", str(cover["id"])))) as image:
                    self.assertEqual(image.convert("RGB").getpixel((image.width // 2, image.height // 2)), (0, 255, 0))
            finally:
                for process in reversed(processes):
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=5)
                log.close()


if __name__ == "__main__":
    unittest.main()
