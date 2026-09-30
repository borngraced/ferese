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
        helpers = script[:script.index('repo_dir=')]
        script = helpers + script[start:script.index('session_target=', start)]
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
                                  env={'repo_dir': str(REPO), 'TEST_ROOT': directory, 'install_root': directory},
                                  capture_output=True, text=True)

    def configuration_preflight(self, portal=None, preferences=None, previous=None, replace=False):
        script = (REPO / 'scripts/install-session.sh').read_text()
        helpers = script[:script.index('repo_dir=')]
        start = script.index('for entry in ')
        check = script[start:script.index('service=/usr/local/share/dbus-1/services/', start)]
        check = check.replace('/usr/share/xdg-desktop-portal/', '$TEST_ROOT/config/')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative, content in [('portals/ferese.portal', portal), ('ferese-portals.conf', preferences)]:
                if content is not None:
                    destination = root / 'config' / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_text(content)
            if previous:
                for relative, content in previous.items():
                    destination = root / 'current/installer-files/portal' / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_text(content)
            return subprocess.run(['bash', '-eu', '-c', helpers + check],
                                  env={'repo_dir': str(REPO), 'TEST_ROOT': directory, 'install_root': directory,
                                       'replace_portal_config': str(replace).lower()},
                                  capture_output=True, text=True)

    def user_override(self, content, backup=False):
        script = (REPO / 'scripts/install.sh').read_text()
        helpers = script[script.index('check_user_portal_override()'):script.index('fail() {')]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            override = root / 'override.conf'
            if content is not None:
                override.write_text(content.replace('USER_HOME', directory))
            command = 'backup_user_portal_override' if backup else 'check_user_portal_override'
            result = subprocess.run(['bash', '-eu', '-c', helpers +
                                     f'\n{command} "$TEST_ROOT/override.conf" "$TEST_ROOT" test-release'],
                                    env={'TEST_ROOT': directory}, capture_output=True, text=True)
            saved = root / 'override.conf.before-test-release'
            return result, override.exists(), saved.read_text() if saved.exists() else None

    def test_user_development_override_is_backed_up(self):
        content = '[Service]\nExecStart=\nExecStart=USER_HOME/.local/libexec/ferese/xdg-desktop-portal-ferese\n'
        result, exists, saved = self.user_override(content, backup=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(exists)
        self.assertIn('/.local/libexec/ferese/xdg-desktop-portal-ferese', saved)

    def test_custom_user_override_is_preserved_and_reported(self):
        result, exists, saved = self.user_override('[Service]\nExecStart=/custom/portal\n', backup=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(exists)
        self.assertIsNone(saved)
        self.assertIn('takes precedence', result.stderr)

    def test_missing_user_override_needs_no_backup(self):
        result, exists, saved = self.user_override(None, backup=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(exists)
        self.assertIsNone(saved)

    def test_existing_development_install_requires_explicit_replacement(self):
        portal = ('[portal]\nDBusName=org.freedesktop.impl.portal.desktop.ferese\n'
                  'Interfaces=org.freedesktop.impl.portal.ScreenCast;\nUseIn=Ferese;\n')
        preferences = '[preferred]\ndefault=gtk;\norg.freedesktop.impl.portal.ScreenCast=ferese;\n'
        result = self.configuration_preflight(portal, preferences)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('--replace-portal-config', result.stderr)
        result = self.configuration_preflight(portal, preferences, replace=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_previous_portal_configuration_is_backed_up(self):
        script = (REPO / 'scripts/install-session.sh').read_text()
        start = script.index('for relative in portals/ferese.portal')
        backup = script[start:script.index('for relative in ', start + 1)]
        backup = backup.replace('/usr/share/xdg-desktop-portal/', '$TEST_ROOT/config/')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative in ('portals/ferese.portal', 'ferese-portals.conf'):
                source = root / 'config' / relative
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text('existing custom content\n')
            result = subprocess.run(['bash', '-eu', '-c', backup],
                                    env={'TEST_ROOT': directory, 'release_dir': str(root / 'release')},
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            for relative in ('portals/ferese.portal', 'ferese-portals.conf'):
                self.assertEqual((root / 'release/portal-config.previous' / relative).read_text(),
                                 'existing custom content\n')
                self.assertEqual((root / 'config' / relative).read_text(), 'existing custom content\n')

    def test_upgrades_recorded_previous_release(self):
        old_portal = '[portal]\nInterfaces=org.freedesktop.impl.portal.Settings;\n'
        old_preferences = '[preferred]\norg.freedesktop.impl.portal.Settings=ferese;\n'
        result = self.configuration_preflight(old_portal, old_preferences,
                                              {'ferese.portal': old_portal, 'ferese-portals.conf': old_preferences})
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.configuration_preflight(old_portal + 'UseIn=Custom;\n', old_preferences,
                                              {'ferese.portal': old_portal, 'ferese-portals.conf': old_preferences})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unmanaged portal configuration', result.stderr)

    def test_preserves_custom_configuration(self):
        portal = (REPO / 'packaging/portal/ferese.portal').read_text()
        preferences = '[preferred]\ndefault=custom;\n'
        result = self.configuration_preflight(portal, preferences)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unmanaged portal configuration', result.stderr)

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
