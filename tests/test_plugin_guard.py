import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from nivora_launcher.plugin_guard import PluginOperationGuard, MAX_CALLS, MAX_BYTES_PER_MINUTE
from nivora_launcher.errors import LauncherError
from nivora_launcher.plugins import _validate_v2_ui_node

class PluginGuardTests(unittest.TestCase):
    def test_call_rate_enforced_and_recovers(self):
        guard = PluginOperationGuard()
        with patch('nivora_launcher.plugin_guard.time.monotonic', return_value=10):
            for _ in range(MAX_CALLS): guard.consume('a', {})
            with self.assertRaises(LauncherError): guard.consume('a', {})
            guard.consume('b', {})
            self.assertTrue(guard.usage('a')['enforced'])
        with patch('nivora_launcher.plugin_guard.time.monotonic', return_value=71):
            guard.consume('a', {})
            self.assertEqual(guard.usage('a')['calls'], 1)

    def test_byte_budget_and_payload(self):
        guard = PluginOperationGuard()
        for _ in range(7): guard.consume('a', {'body': 'a' * (1024 * 1024)})
        with self.assertRaises(LauncherError): guard.consume('a', {'body': 'a' * (1024 * 1024)})
        with self.assertRaises(LauncherError): guard.consume('b', {'body': 'a' * (5 * 1024 * 1024)})
        self.assertLess(guard.usage('a')['bytes'], MAX_BYTES_PER_MINUTE)

    def test_audit_never_records_values(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / 'audit'
            PluginOperationGuard().audit(root, 'a', 'secrets.store', {'value': 'MY-SECRET', 'url': 'https://test/?token=MY-SECRET'}, 'denied', 2)
            text = (root / 'plugin-operations.jsonl').read_text()
            self.assertNotIn('MY-SECRET', text)
            data = json.loads(text)
            self.assertEqual(data['argumentKeys'], ['url', 'value'])
            self.assertEqual(data['operation'], 'secrets.store')
            self.assertEqual((root / 'plugin-operations.jsonl').stat().st_mode & 0o777, 0o600)

    def test_audit_link_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original = root / 'secret'; original.write_text('untouched')
            (root / 'plugin-operations.jsonl').symlink_to(original)
            with self.assertRaises(LauncherError): PluginOperationGuard().audit(root, 'a', 'op', {}, 'ok', 1)
            self.assertEqual(original.read_text(), 'untouched')

    def test_ui_nodes_and_depth(self):
        nodes = [
            {'type':'form','id':'form','fields':[{'type':'input','id':'name','label':'Name','onChange':'changed'}],'submitAction':'submit'},
            {'type':'tabs','id':'tabs','tabs':[{'id':'one','label':'One','content':{'type':'text','text':'Hello'}}]},
            {'type':'accordion','id':'a','sections':[{'id':'one','label':'One','content':{'type':'text','text':'Hello'}}]},
            {'type':'table','id':'t','columns':[{'key':'name','label':'Name'}],'rows':[{'name':'value'}]},
            {'type':'chart','id':'c','values':[1,2,3]},
            {'type':'include','id':'i','fragment':'one'},
        ]
        for node in nodes: _validate_v2_ui_node(node, 'test', 0, [0])
        node = {'type':'text','text':'leaf'}
        for _ in range(11): node = {'type':'group','children':[node]}
        with self.assertRaises(LauncherError): _validate_v2_ui_node(node, 'test', 0, [0])
        with self.assertRaises(LauncherError): _validate_v2_ui_node({'type':'chart','id':'c','values':[float('nan')]}, 'test', 0, [0])
        with self.assertRaises(LauncherError): _validate_v2_ui_node({'type':'list','items':['x'] * 513}, 'test', 0, [0])

    def test_ui_v2_extended_nodes(self):
        nodes = [
            {'type':'textarea','id':'notes','label':'Notes','value':'x','rows':6},
            {'type':'slider','id':'fps','label':'FPS','min':30,'max':144,'step':6,'value':60,'unit':'fps','config_key':'fps'},
            {'type':'number','id':'count','label':'Count','value':5,'min':0,'max':10,'unit':'шт','onChange':'changed'},
            {'type':'color','id':'accent','label':'Accent','value':'#8b653f'},
            {'type':'datetime','id':'when','label':'Когда','value':'2026-01-01T10:00'},
            {'type':'switch','id':'auto','label':'Авто','checked':True,'hint':'Подсказка'},
            {'type':'metric','id':'players','label':'Игроки','value':'12','unit':'онлайн','tone':'success'},
            {'type':'alert','id':'warn','title':'Внимание','text':'Проверьте настройки','tone':'warning'},
            {'type':'code','id':'snippet','text':'say hello'},
            {'type':'image','id':'logo','url':'https://example.com/l.png','alt':'logo','height':120},
            {'type':'card','id':'box','label':'Блок','tone':'muted','children':[{'type':'text','text':'внутри'}]},
            {'type':'grid','id':'g','columns':3,'children':[{'type':'badge','label':'1'},{'type':'badge','label':'2'}]},
        ]
        for node in nodes:
            _validate_v2_ui_node(node, 'test', 0, [0])
        rejects = [
            {'type':'slider','id':'fps','label':'FPS','min':100,'max':10},
            {'type':'color','id':'c','label':'C','value':'12zz34'},
            {'type':'textarea','id':'t','label':'T','rows':99},
            {'type':'grid','id':'g','columns':9,'children':[]},
            {'type':'switch','id':'s','label':'S','checked':'yes'},
            {'type':'metric','id':'m','label':'M'},
            {'type':'unknown-new','id':'x'},
        ]
        for node in rejects:
            with self.assertRaises(LauncherError): _validate_v2_ui_node(node, 'test', 0, [0])
