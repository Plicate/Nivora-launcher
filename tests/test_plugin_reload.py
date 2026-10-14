import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from nivora_launcher import plugin_reload
from nivora_launcher.cli import build_parser
from nivora_launcher.errors import LauncherError

class PluginReloadTests(unittest.TestCase):
    def config(self, root, enabled=True):
        return SimpleNamespace(paths=SimpleNamespace(data_home=root), settings=SimpleNamespace(developer_mode=enabled))

    def test_reload_requires_developer_mode_and_trust(self):
        with tempfile.TemporaryDirectory() as folder:
            config = self.config(Path(folder), False)
            with self.assertRaises(LauncherError): plugin_reload.request_reload('test', config)
            config.settings.developer_mode = True
            with patch.object(plugin_reload, 'PluginStore') as store:
                store.return_value.runtime_source.side_effect = LauncherError('Integrity changed')
                with self.assertRaises(LauncherError): plugin_reload.request_reload('test', config)
                self.assertFalse((config.paths.data_home / 'plugin-dev-reload.json').exists())

    def test_approved_reload_writes_marker_and_cli_parses(self):
        with tempfile.TemporaryDirectory() as folder:
            config = self.config(Path(folder))
            with patch.object(plugin_reload, 'PluginStore') as store:
                store.return_value.runtime_source.return_value = {'source': 'approved'}
                marker = plugin_reload.request_reload('test', config)
                store.return_value.runtime_source.assert_called_once_with('test')
            self.assertEqual(plugin_reload._requests(config), {'test': marker})
            args = build_parser().parse_args(['plugin', 'reload', 'test'])
            self.assertEqual(args.plugin_id, 'test')

    def test_watcher_tracks_actual_entrypoint_and_rejects_links(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); directory = root / 'plugins' / 'test'; directory.mkdir(parents=True)
            (directory / 'plugin.json').write_text(json.dumps({'entrypoint': 'bundle.js'}))
            source = directory / 'bundle.js'; source.write_text('one')
            config = self.config(root)
            store = SimpleNamespace(root=root / 'plugins', list=lambda: [{'id':'test','installed':True}])
            with patch.object(plugin_reload, 'PluginStore', return_value=store):
                before = plugin_reload.changes(config)
                source.write_text('changed source')
                self.assertNotEqual(before['generation'], plugin_reload.changes(config)['generation'])
                source.unlink(); source.symlink_to(directory / 'plugin.json')
                with self.assertRaises(LauncherError): plugin_reload.changes(config)

    def test_invalid_reload_registry_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            config = self.config(Path(folder)); path = config.paths.data_home / 'plugin-dev-reload.json'
            path.write_text(json.dumps({'../escape': 1}))
            with self.assertRaises(LauncherError): plugin_reload._requests(config)
