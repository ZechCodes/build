"""The discovery control can only alter its explicitly selected disposable DB."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("qualification_discovery", Path(__file__).with_name("discovery.py"))
discovery = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(discovery)


class DiscoveryBoundary(unittest.TestCase):
    def test_selected_task_root_is_used_and_database_symlink_escape_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            run_dir = scratch / "run-fixture"
            run_dir.mkdir()
            database = run_dir / "app.db"
            database.touch()
            fixture = run_dir / "runtime.json"
            fixture.write_text(json.dumps({"runDir": str(run_dir),
                "hostedOrigin": "https://localhost:18478", "bridgeOrigin": "https://localhost:19478"}))
            self.assertEqual(discovery.load_fixture(fixture, scratch)[1], run_dir)
            with self.assertRaises(ValueError):
                discovery.load_fixture(fixture, scratch / "other-task")
            outside = scratch / "outside.db"
            outside.touch()
            database.unlink()
            database.symlink_to(outside)
            with self.assertRaises(ValueError):
                discovery.load_fixture(fixture, scratch)
