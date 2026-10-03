"""Exercise real shell frame-driven motion on a private nested desktop and bus.

FERESE_TEST_SHELL_MOTION=1 python3 scripts/tests/test_shell_motion_isolated.py
Uses release binaries by default. Requires a Wayland host, dbus-daemon and gdbus.
Protocol traces contain only this test's shell traffic, never the host desktop.
"""
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("FERESE_TEST_SHELL_MOTION") == "1", "opt-in nested shell test")
class ShellMotionTest(unittest.TestCase):
    def test_notifications_and_modals_follow_frames_and_finish_closing(self):
        repo = Path(__file__).resolve().parents[2]
        binary = repo / os.environ.get("FERESE_TEST_BINARY", "target/release/ferese")
        shell = repo / os.environ.get("FERESE_TEST_SHELL", "target/release/ferese-shell")
        ctl = repo / os.environ.get("FERESE_TEST_CTL", "target/release/feresectl")
        children = []
        with tempfile.TemporaryDirectory(prefix="ferese-shell-motion-") as temporary:
            root = Path(temporary)
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            config = root / "config/ferese"
            config.mkdir(parents=True)
            (config / "config.kdl").write_text(
                'animations { speed 0.5; }\nstatus { keybinding-guide #false; }\n')
            display = Path(os.environ["WAYLAND_DISPLAY"])
            if not display.is_absolute():
                display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
            env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), WAYLAND_DISPLAY=str(display),
                       XDG_CONFIG_HOME=str(root / "config"), XDG_STATE_HOME=str(root / "state"),
                       XDG_CACHE_HOME=str(root / "cache"), RUST_LOG="ferese=info,ferese::nested_input=debug")
            env.pop("WAYLAND_SOCKET", None)
            env.pop("FERESE_SHELL_CONTROL_SOCKET", None)
            log_path = root / "session.log"
            # No service directories: readiness checks must not auto-activate
            # the host's notification daemon on this private bus.
            bus_config = root / "bus.conf"
            bus_config.write_text(
                '<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
                '<policy context="default"><allow send_destination="*"/>'
                '<allow receive_sender="*"/><allow own="*"/></policy></busconfig>')

            def launch(command, **kwargs):
                process = subprocess.Popen(command, env=env, start_new_session=True, **kwargs)
                children.append(process)
                return process

            def wait_for(predicate, timeout=15):
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    value = predicate()
                    if value:
                        return value
                    time.sleep(.025)
                trace = log_path.read_text()
                relevant = '\n'.join(line for line in trace.splitlines() if any(
                    word in line for word in ('get_layer_surface', 'ERROR', 'WARN', 'notification')))
                self.fail("Timed out:\n" + relevant[-16000:] + '\n' + trace[-4000:])

            def dbus(method, *args, check=True):
                return subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.Notifications",
                                       "--object-path", "/org/freedesktop/Notifications", "--method",
                                       "org.freedesktop.Notifications." + method, *args],
                                      env=env, text=True, capture_output=True, timeout=5, check=check)

            def layer_surface(namespace):
                matches = list(re.finditer(r'get_layer_surface\([^\n]*?wl_surface[@#](\d+)[^\n]*"' + namespace + '"', log_path.read_text()))
                # Wayland object IDs can be reused after destruction. Restrict
                # assertions to this surface's lifetime in the trace.
                return (matches[-1].group(1), matches[-1].start()) if matches else None

            def frames(surface):
                identity, start = surface
                return len(re.findall(r'wl_surface[@#]' + identity + r'\.frame\(', log_path.read_text()[start:]))

            def commits(surface):
                identity, start = surface
                return len(re.findall(r'wl_surface[@#]' + identity + r'\.commit\(', log_path.read_text()[start:]))

            def destroyed(surface):
                identity, start = surface
                return re.search(r'wl_surface[@#]' + identity + r'\.destroy\(', log_path.read_text()[start:]) is not None

            with log_path.open("w") as log:
                try:
                    bus = launch(["dbus-daemon", "--config-file=" + str(bus_config), "--nofork", "--print-address=1"],
                                 stdout=subprocess.PIPE, stderr=log, text=True)
                    env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
                    compositor = launch([str(binary), "--backend", "nested", "--grant-effects", "--grant-shell-control",
                                         "--", "env", "WAYLAND_DEBUG=client", "ICED_BACKEND=tiny-skia", str(shell)],
                                        stdout=log, stderr=log)
                    wait_for(lambda: (runtime / "ferese/control.sock").is_socket())
                    wait_for(lambda: dbus("GetServerInformation", check=False).returncode == 0)
                    wait_for(lambda: layer_surface("ferese-shell-top-bar"))
                    time.sleep(.5)
                    reply = dbus("Notify", "Ferese motion test", "0", "", "Frame cadence", "Synthetic test notification", "[]", "{}", "0").stdout
                    notice = re.search(r'uint32 (\d+)', reply).group(1)
                    surface = wait_for(lambda: layer_surface("ferese-shell-notifications"))
                    time.sleep(1.5)
                    opening_frames = frames(surface)
                    self.assertGreater(opening_frames, 10, log_path.read_text()[-8000:])
                    time.sleep(1)
                    for attempt in range(3):
                        trace_start = len(log_path.read_text())
                        before = commits(surface)
                        time.sleep(2)
                        idle_frames = commits(surface) - before
                        if 'ferese::nested_input:' not in log_path.read_text()[trace_start:]:
                            break
                    else:
                        self.fail("Host input interrupted all three idle measurement intervals")
                    # Clock/status updates still redraw settled surfaces. Count
                    # commits: Iced can request two callbacks for one redraw.
                    self.assertLessEqual(idle_frames, 12, "settled toast kept requesting animation frames")
                    material_updates = log_path.read_text().count('.set_region_opacities(')
                    dbus("CloseNotification", notice)
                    wait_for(lambda: destroyed(surface), timeout=5)
                    self.assertGreater(log_path.read_text().count('.set_region_opacities(') - material_updates, 5,
                                       "notification materials did not follow their fade")
                    print(f"Toast: {opening_frames} opening callback requests, {idle_frames / 2:g} settled commits/s; destroyed", flush=True)

                    subprocess.run([str(ctl), "toggle-keybinding-guide"], env=env, check=True, capture_output=True)
                    modal = wait_for(lambda: layer_surface("ferese-system-modal"))
                    wait_for(lambda: frames(modal) >= 4)
                    time.sleep(.3)
                    subprocess.run([str(ctl), "toggle-keybinding-guide"], env=env, check=True, capture_output=True)
                    wait_for(lambda: destroyed(modal), timeout=5)
                    self.assertGreater(frames(modal), 10, "modal close did not follow frames")
                    self.assertIsNone(compositor.poll(), log_path.read_text()[-8000:])
                    self.assertNotIn("panicked at", log_path.read_text())
                    print(f"Modal: {frames(modal)} callbacks; interrupted opening closed and destroyed", flush=True)
                finally:
                    for child in reversed(children):
                        if child.poll() is None:
                            os.killpg(child.pid, signal.SIGTERM)
                            try:
                                child.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                os.killpg(child.pid, signal.SIGKILL)
                                child.wait(timeout=5)
                        if child.stdout is not None:
                            child.stdout.close()


if __name__ == "__main__":
    unittest.main()
