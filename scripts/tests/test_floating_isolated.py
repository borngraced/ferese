"""Opt-in rendered regression test; opens only a temporary nested compositor.
FERESE_TEST_FLOATING=1 FERESE_TEST_BINARY=target/debug/ferese python3 scripts/tests/test_floating_isolated.py
Requires cc, pkg-config, wayland-scanner, wayland-protocols, grim, and Pillow.
"""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("FERESE_TEST_FLOATING") == "1", "nested render test is opt-in")
class FloatingSizeTest(unittest.TestCase):
    def test_committed_content_is_not_cropped_to_requested_size(self):
        from PIL import Image, ImageChops
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="ferese-floating-test-") as tmp:
            root = Path(tmp)
            protocols = subprocess.check_output(
                ["pkg-config", "--variable=pkgdatadir", "wayland-protocols"], text=True
            ).strip()
            xml = str(Path(protocols) / "stable/xdg-shell/xdg-shell.xml")
            for kind, output in [("client-header", "xdg-shell-client-protocol.h"),
                                 ("private-code", "xdg-shell-protocol.c")]:
                subprocess.run(["wayland-scanner", kind, xml, str(root / output)], check=True)
            flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "wayland-client"], text=True).split()
            subprocess.run(["cc", "-Wall", "-Wextra", "-I", str(root),
                            str(repo / "scripts/tests/fixtures/floating-size.c"),
                            str(root / "xdg-shell-protocol.c"), "-o", str(root / "client"), *flags], check=True)
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            config = root / "config/ferese"
            config.mkdir(parents=True)
            (config / "config.kdl").write_text(
                'window-rule app-id="ferese.test.floating-size" floating=#true width=320 height=240\n'
                'theme {\n geometry {\n border-width 0\n focus-ring-width 0\n window-radius 0\n }\n}\n'
            )
            display = os.environ["WAYLAND_DISPLAY"]
            if not display.startswith("/"):
                display = str(Path(os.environ["XDG_RUNTIME_DIR"]) / display)
            env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(root / "config"),
                       WAYLAND_DISPLAY=display, FERESE_ENABLE_SCREENCOPY="1")
            binary = repo / os.environ.get("FERESE_TEST_BINARY", "target/debug/ferese")
            with (root / "compositor.log").open("w") as log:
                process = subprocess.Popen([str(binary), "--backend", "nested", "--", str(root / "client")],
                                           env=env, stdout=log, stderr=subprocess.STDOUT)
                try:
                    for _ in range(100):
                        if process.poll() is not None:
                            self.fail((root / "compositor.log").read_text())
                        sockets = [p for p in runtime.glob("wayland-*") if not p.name.endswith(".lock")]
                        if sockets:
                            break
                        time.sleep(.1)
                    self.assertTrue(sockets, "nested Wayland socket did not appear")
                    childenv = dict(env, WAYLAND_DISPLAY=str(sockets[0]))
                    time.sleep(2)
                    screenshot = root / "frame.png"
                    subprocess.run(["grim", "-s", "1", str(screenshot)], env=childenv, check=True, timeout=15)
                    with Image.open(screenshot) as frame:
                        rgb = frame.convert("RGB")
                        red, green, blue = rgb.split()
                        mask = ImageChops.multiply(
                            ImageChops.multiply(red.point(lambda v: 255 if v < 60 else 0),
                                                green.point(lambda v: 255 if v > 180 else 0)),
                            blue.point(lambda v: 255 if v < 100 else 0),
                        )
                        bounds = mask.getbbox()
                    self.assertIsNotNone(bounds, "client content was not rendered")
                    # The shader antialiases the outermost pixel even at radius zero.
                    for actual, expected in zip((bounds[2] - bounds[0], bounds[3] - bounds[1]), (640, 480)):
                        self.assertTrue(expected - 2 <= actual <= expected,
                                        f"floating content cropped: {bounds}, expected 640x480")
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == "__main__":
    unittest.main()
