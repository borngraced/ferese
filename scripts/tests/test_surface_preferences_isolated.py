"""FERESE_TEST_SURFACE_PREFERENCES=1 python3 scripts/tests/test_surface_preferences_isolated.py"""

import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("FERESE_TEST_SURFACE_PREFERENCES") == "1", "opt-in nested test")
class SurfacePreferencesTest(unittest.TestCase):
    def test_v6_preferences_and_v5_compatibility(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="ferese-surface-preferences-") as temporary:
            root = Path(temporary)
            protocols = subprocess.check_output(
                ["pkg-config", "--variable=pkgdatadir", "wayland-protocols"], text=True
            ).strip()
            xml = str(Path(protocols) / "staging/fractional-scale/fractional-scale-v1.xml")
            for kind, name in [("client-header", "fractional-scale-client-protocol.h"),
                               ("private-code", "fractional-scale.c")]:
                subprocess.run(["wayland-scanner", kind, xml, str(root / name)], check=True)
            flags = subprocess.check_output(
                ["pkg-config", "--cflags", "--libs", "wayland-client"], text=True
            ).split()
            subprocess.run([
                "cc", "-Wall", "-Wextra", "-Werror", "-I", str(root),
                str(repo / "scripts/tests/fixtures/surface-preferences.c"),
                str(root / "fractional-scale.c"), "-o", str(root / "client"), *flags,
            ], check=True)
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            config = root / "config/ferese"
            config.mkdir(parents=True)
            (config / "config.kdl").write_text("\n")
            display = Path(os.environ["WAYLAND_DISPLAY"])
            if not display.is_absolute():
                display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
            env = dict(os.environ, WAYLAND_DISPLAY=str(display), XDG_RUNTIME_DIR=str(runtime),
                       XDG_CONFIG_HOME=str(root / "config"))
            binary = repo / os.environ.get("FERESE_TEST_BINARY", "target/debug/ferese")
            with (root / "compositor.log").open("w") as log:
                compositor = subprocess.Popen([str(binary), "--backend", "nested"],
                                              env=env, stdout=log, stderr=log)
                try:
                    sockets = []
                    for _ in range(100):
                        if compositor.poll() is not None:
                            self.fail((root / "compositor.log").read_text())
                        sockets = [p for p in runtime.glob("wayland-*") if p.is_socket()]
                        if sockets:
                            break
                        time.sleep(0.1)
                    self.assertTrue(sockets, "nested compositor did not create a socket")
                    for version in [5, 6]:
                        subprocess.run([str(root / "client"), str(version)], check=True, timeout=10,
                                       env=dict(env, WAYLAND_DISPLAY=str(sockets[0])))
                finally:
                    compositor.terminate()
                    try:
                        compositor.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        compositor.kill()
                        compositor.wait()


if __name__ == "__main__":
    unittest.main()
