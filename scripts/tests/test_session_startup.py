"""Session startup regressions using fake services; never touches the host bus."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / 'packaging/ferese-session-shell'


class SessionStartupTest(unittest.TestCase):
    def run_session(self, direct=True, fail_target=False, shell_status=0, terminate=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / 'ferese-session-shell'
            shutil.copyfile(SCRIPT, helper)
            programs = {
                'dbus-update-activation-environment': 'printf "import:%s\\n" "$WAYLAND_DISPLAY" >> "$TEST_ROOT/events"\n',
                'dbus-send': 'echo reload-bus >> "$TEST_ROOT/events"\n',
                'systemctl': 'echo "$*" >> "$TEST_ROOT/events"\nif [[ $* == "--user start ferese-session.target" && $TEST_FAIL == 1 ]]; then exit 1; fi\n',
                'ferese-polkit-agent': 'exec sleep 30\n',
                'ferese-shell': 'printf "shell:%s\\n" "$WAYLAND_SOCKET" >> "$TEST_ROOT/events"\nif [[ $TEST_WAIT == 1 ]]; then exec sleep 30; fi\nexit "$TEST_STATUS"\n',
            }
            for name, body in programs.items():
                path = root / name
                path.write_text('#!/bin/bash\n' + body)
                path.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', TEST_ROOT=directory,
                       FERESE_SESSION_IMPORT_ENV=str(int(direct)), WAYLAND_DISPLAY='private-display',
                       FERESE_PUBLIC_WAYLAND_DISPLAY='public-display', WAYLAND_SOCKET='77',
                       TEST_FAIL=str(int(fail_target)), TEST_STATUS=str(shell_status), TEST_WAIT=str(int(terminate)))
            child = subprocess.Popen(['bash', str(helper)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if terminate:
                deadline = time.monotonic() + 3
                while not (root / 'events').exists() or 'shell:77' not in (root / 'events').read_text():
                    if time.monotonic() > deadline:
                        child.kill()
                        self.fail('Session shell did not start')
                    time.sleep(.01)
                child.send_signal(signal.SIGTERM)
            child.communicate(timeout=4)
            return child.returncode, (root / 'events').read_text().splitlines()

    def test_direct_session_imports_public_display_before_activation_and_cleans_up(self):
        status, events = self.run_session(shell_status=7)
        self.assertEqual(status, 7)
        self.assertEqual(events, ['import:public-display', 'reload-bus', '--user daemon-reload',
                                  '--user start ferese-session.target',
                                  '--user try-restart xdg-desktop-portal-ferese.service', 'shell:77',
                                  '--user stop ferese-session.target'])

    def test_nested_session_never_changes_host_activation(self):
        status, events = self.run_session(direct=False)
        self.assertEqual(status, 0)
        self.assertEqual(events, ['shell:77'])

    def test_failed_target_does_not_prevent_shell_or_stop_unowned_target(self):
        status, events = self.run_session(fail_target=True)
        self.assertEqual(status, 0)
        self.assertIn('shell:77', events)
        self.assertNotIn('--user stop ferese-session.target', events)
        self.assertNotIn('--user try-restart xdg-desktop-portal-ferese.service', events)

    def test_termination_stops_the_session_target(self):
        status, events = self.run_session(terminate=True)
        self.assertEqual(status, 143)
        self.assertEqual(events[-1], '--user stop ferese-session.target')


if __name__ == '__main__':
    unittest.main()
