"""Consumer-review witnesses: explicit research gaps, not production adapters."""
import unittest

from prototype import SelectionDB
from test_prototype import header


def ascii_fold(value):
    """Test-only input adapter; the research service itself does not normalize."""
    return value.translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"))


class ConsumerDomainWitnesses(unittest.TestCase):
    def selected_total(self, source_label, selected_label, number=1):
        db = SelectionDB(soft_ms=float("inf"))
        try:
            db.put_task(header(number), [source_label])
            def stage(kind, values):
                token = db.create_set(kind, len(values))
                db.stage_set(token, values)
                db.seal_set(token)
                return token
            build = db.start("p", roster=stage("roster", []),
                             labels=stage("labels", [selected_label]),
                             workspaces=None, statuses=stage("statuses", ["backlog"]))
            for _ in range(4):
                if db.quantum(build) == "sealed":
                    return db.summary(build)["total"]
            self.fail("small witness did not seal")
        finally:
            db.connection.close()

    def test_raw_intersection_does_not_implement_ascii_normalization(self):
        self.assertEqual(self.selected_total("API", "api"), 0)

    def test_declared_prenormalized_label_domain_witnesses(self):
        for source, selected, expected in [
            ("API", "api", 1), ("É", "é", 0), (" api ", "api", 0),
            ("API\0v2", "api\0v2", 1), ("x\0y", "x\ufffdy", 0),
            ("\0", "\0", 1), ("\ufffd", "\ufffd", 1),
        ]:
            with self.subTest(source=source, selected=selected):
                self.assertEqual(self.selected_total(ascii_fold(source), ascii_fold(selected)), expected)

    def test_i64_prototype_acceptance_is_not_js_consumer_support(self):
        maximum = 2**53 - 1
        self.assertEqual(self.selected_total("a", "a", maximum), 1)
        self.assertEqual(self.selected_total("a", "a", maximum + 2), 1)
        # Binary64 cannot distinguish these canonical integer identities. This
        # witnesses why the proposed numeric wire must refuse the larger one.
        self.assertEqual(float(maximum + 1), float(maximum + 2))


if __name__ == "__main__":
    unittest.main()
