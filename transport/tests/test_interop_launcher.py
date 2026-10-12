"""Exercise the JS subprocess contract without crypto dependencies or a venv."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'transport/js/test/helpers/python.mjs'


class PythonLauncherContract(unittest.TestCase):
    def invoke(self, **env):
        return subprocess.run(
            ['node', '--input-type=module', '-e',
             f'import {{ python }} from {json.dumps(LAUNCHER.as_uri())}; '
             'console.log(JSON.stringify(python({action: "probe"})));'],
            cwd='/', env={**os.environ, **env}, text=True,
            capture_output=True, check=False,
        )

    def test_default_is_in_tree_frozen_and_independent_of_cwd(self):
        with tempfile.TemporaryDirectory() as directory:
            uv = Path(directory) / 'fake-uv'
            uv.write_text('#!/usr/bin/env python\nimport json, os, sys\n'
                          'json.dump({"args": sys.argv[1:], "path": os.environ["PYTHONPATH"], '
                          '"input": json.load(sys.stdin)}, sys.stdout)\n')
            uv.chmod(0o755)
            result = self.invoke(BST_UV=str(uv), BUILD_SECURE_TRANSPORT_PY='')
            self.assertEqual(result.returncode, 0, result.stderr)
            response = json.loads(result.stdout)
            reference = ROOT / 'transport/python'
            self.assertEqual(response['args'], ['run', '--frozen', '--quiet', '--project',
                             str(reference), 'python', str(reference / 'tests/interop_helper.py')])
            self.assertEqual(response['path'], str(reference / 'src'))
            self.assertEqual(response['input'], {'action': 'probe'})

    def test_missing_tool_fails_with_actionable_message(self):
        result = self.invoke(BST_UV='/definitely-missing/uv', BUILD_SECURE_TRANSPORT_PY='')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('required uv executable', result.stderr)

    def test_missing_reference_fails_before_launch(self):
        result = self.invoke(BUILD_SECURE_TRANSPORT_PY='/definitely-missing/reference')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('required Python reference', result.stderr)

    def test_failed_helper_cannot_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            uv = Path(directory) / 'fake-uv'
            uv.write_text('#!/bin/sh\necho fixture-failure >&2\nexit 7\n')
            uv.chmod(0o755)
            result = self.invoke(BST_UV=str(uv), BUILD_SECURE_TRANSPORT_PY='')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('fixture-failure', result.stderr)
            self.assertIn('exit 7', result.stderr)
