#!/usr/bin/env python3
"""Exercise screenshot selection and locking without a Wayland desktop."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / 'packaging/ferese-screenshot'


class ScreenshotTest(unittest.TestCase):
    def test_active_output_all_outputs_and_editor_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bins = root / 'bin'
            bins.mkdir()
            env = dict(os.environ, XDG_RUNTIME_DIR=directory, TEST_ROOT=directory,
                       PATH=f'{bins}:{os.environ["PATH"]}')
            (root / 'outputs.json').write_text(json.dumps([
                {'name': 'eDP-1', 'enabled': True, 'focused': False},
                {'name': 'HDMI-A-1', 'enabled': True, 'focused': True},
            ]))
            scripts = {
                'grim': '''#!/bin/bash
printf '%s\\n' "$@" >> "$TEST_ROOT/captures"
printf image > "${@: -1}"
''',
                'feresectl': '#!/bin/bash\ncat "$TEST_ROOT/outputs.json"\n',
                'notify-send': '#!/bin/bash\nexit 0\n',
                'wl-copy': '#!/bin/bash\nexit 0\n',
                'xdg-user-dir': '#!/bin/bash\nprintf "%s" "$TEST_ROOT/Pictures"\n',
                'satty': '''#!/bin/bash
if [[ -e /proc/$$/fd/9 ]]; then touch "$TEST_ROOT/inherited-lock"; fi
printf 'editor\\n' >> "$TEST_ROOT/editors"
touch "$TEST_ROOT/ready"
while [[ ! -e "$TEST_ROOT/release" ]]; do sleep 0.02; done
''',
            }
            for name, contents in scripts.items():
                path = bins / name
                path.write_text(contents)
                path.chmod(0o755)
            def run(mode):
                return subprocess.run(['bash', str(SCRIPT), mode], env=env,
                                      capture_output=True, timeout=3)
            first = subprocess.Popen(['bash', str(SCRIPT), '--full'], env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 5
                while not (root / 'ready').exists():
                    if time.monotonic() > deadline or first.poll() is not None:
                        self.fail('First screenshot did not open its editor')
                    time.sleep(0.02)
                second = run('--full')
                self.assertEqual(second.returncode, 0, second.stderr)
                self.assertEqual((root / 'editors').read_text().splitlines(), ['editor'])
                self.assertFalse((root / 'inherited-lock').exists())
                (root / 'release').touch()
                _, stderr = first.communicate(timeout=3)
                self.assertEqual(first.returncode, 0, stderr)
                third = run('--full')
                self.assertEqual(third.returncode, 0, third.stderr)
                self.assertEqual((root / 'editors').read_text().splitlines(), ['editor', 'editor'])
                self.assertFalse(list((root / 'Pictures/Screenshots').glob('*.png')))
                self.assertFalse(list(root.glob('ferese-screenshot-*.png')))
                captures = (root / 'captures').read_text().splitlines()
                self.assertEqual(captures[0:2], ['-o', 'HDMI-A-1'])
                self.assertEqual(captures[3:5], ['-o', 'HDMI-A-1'])
                all_monitors = run('--all')
                self.assertEqual(all_monitors.returncode, 0, all_monitors.stderr)
                captures = (root / 'captures').read_text().splitlines()
                self.assertEqual(len(captures), 7)  # --all passes only the filename
                for outputs in [[], [{'enabled': True, 'name': 'A'}, {'enabled': True, 'name': 'B'}]]:
                    (root / 'outputs.json').write_text(json.dumps(outputs))
                    unavailable = run('--full')
                    self.assertNotEqual(unavailable.returncode, 0)
                    self.assertIn(b'Could not determine the active monitor', unavailable.stderr)
                    self.assertEqual((root / 'captures').read_text().splitlines(), captures)
                (root / 'outputs.json').write_text(json.dumps([
                    {'name': 'eDP-1', 'enabled': False, 'focused': True},
                    {'name': 'HDMI-A-1', 'enabled': True, 'focused': False},
                ]))
                single = run('--full')
                self.assertEqual(single.returncode, 0, single.stderr)
                self.assertEqual((root / 'captures').read_text().splitlines()[-3:-1], ['-o', 'HDMI-A-1'])
            finally:
                (root / 'release').touch()
                if first.poll() is None:
                    first.terminate()
                first.communicate(timeout=3)


if __name__ == '__main__':
    unittest.main()
