"""The investigation must prove its initial cache asymmetry, not assume it."""

import unittest
from unittest.mock import patch

from arp_case import configure, validate_initial


class InitialCacheTests(unittest.TestCase):
    def test_cold_rejects_a_phone_warmed_by_signaling(self):
        with self.assertRaisesRegex(AssertionError, "bridge already knows"):
            validate_initial("cold", [{"dst": "10.72.3.254"}], [], [])

    def test_cold_requires_both_caches_empty(self):
        validate_initial("cold", [], [], [])
        with self.assertRaisesRegex(AssertionError, "phone already knows"):
            validate_initial("cold", [], [], [{"dst": "10.72.0.1"}])

    def test_cached_requires_the_intended_one_sided_cache(self):
        for case, state in [("cached", "PERMANENT"), ("refresh", "REACHABLE"), ("stale", "STALE")]:
            with self.subTest(case=case):
                row = {"dst": "10.72.0.1", "state": [state], "lladdr": "02:00:00:00:00:01"}
                validate_initial(case, [], [], [row])
                with self.assertRaisesRegex(AssertionError, "cached MAC missing"):
                    validate_initial(case, [], [], [])
                with self.assertRaisesRegex(AssertionError, "bridge already knows"):
                    validate_initial(case, [{"dst": "10.72.3.254"}], [], [row])

    def test_phone_never_arps_for_bridge_requires_absent_bridge_neighbor(self):
        with self.assertRaisesRegex(AssertionError, "bridge already knows"):
            validate_initial("never-arps", [], [{"dst": "10.72.3.254"}], [])

    def test_production_scouts_remain_enabled_for_cold_and_no_arp_controls(self):
        with patch("arp_case.subprocess.run") as command:
            configure("cold", 123)
            configure("never-arps", 123)
        command.assert_not_called()

    def test_only_named_investigation_control_suppresses_scouts(self):
        with patch("arp_case.subprocess.run") as command:
            configure("suppressed", 123)
        self.assertEqual(command.call_args.args, (["nft", "-f", "-"],))


if __name__ == "__main__":
    unittest.main()
