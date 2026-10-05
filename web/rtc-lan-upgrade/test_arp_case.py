"""The investigation must prove its initial cache asymmetry, not assume it."""

import unittest

from arp_case import validate_initial


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

    def test_proxy_entries_cannot_pass_as_absent(self):
        with self.assertRaisesRegex(AssertionError, "bridge already knows"):
            validate_initial("proxy", [], [{"dst": "10.72.3.254"}], [])


if __name__ == "__main__":
    unittest.main()
