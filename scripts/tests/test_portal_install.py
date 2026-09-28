"""Exercise portal upgrade preflight against temporary installation files."""
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
LEGACY = ('[D-BUS Service]\n'
          'Name=org.freedesktop.impl.portal.desktop.ferese\n'
          'Exec=/usr/local/lib/ferese/current/xdg-desktop-portal-ferese\n')


class PortalInstallTest(unittest.TestCase):
    def preflight(self, service=None, unit=None):
        script = (REPO / 'scripts/install-session.sh').read_text()
        start = script.index('service=/usr/local/share/dbus-1/services/')
        script = script[start:script.index('session_target=', start)]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            if service is not None:
                (root / 'activation.service').write_text(service)
            if unit is not None:
                (root / 'portal.service').write_text(unit)
            script = script.replace(
                'service=/usr/local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.ferese.service',
                'service="$TEST_ROOT/activation.service"')
            script = script.replace('$(basename "$service")',
                                    'org.freedesktop.impl.portal.desktop.ferese.service')
            script = script.replace(
                'portal_unit=/usr/local/lib/systemd/user/xdg-desktop-portal-ferese.service',
                'portal_unit="$TEST_ROOT/portal.service"')
            return subprocess.run(['bash', '-eu', '-c', script],
                                  env={'repo_dir': str(REPO), 'TEST_ROOT': directory},
                                  capture_output=True, text=True)

    def test_accepts_fresh_install_and_previous_activation_file(self):
        self.assertEqual(self.preflight().returncode, 0)
        self.assertEqual(self.preflight(LEGACY).returncode, 0)

    def test_accepts_current_managed_files(self):
        service = (REPO / 'packaging/portal/org.freedesktop.impl.portal.desktop.ferese.service').read_text()
        unit = (REPO / 'packaging/systemd/xdg-desktop-portal-ferese.service').read_text()
        self.assertEqual(self.preflight(service, unit).returncode, 0)

    def test_preserves_modified_activation_and_systemd_files(self):
        result = self.preflight(LEGACY.replace('/usr/local/lib/ferese/current/', '/custom/'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unmanaged portal service', result.stderr)
        result = self.preflight(LEGACY, '[Service]\nExecStart=/custom/portal\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unmanaged portal unit', result.stderr)


if __name__ == '__main__':
    unittest.main()
