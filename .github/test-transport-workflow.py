"""Transport CI contracts; run with Python's standard library, without CI dispatch.

This deliberately recognizes only the workflow's current job/step layout. YAML
syntax belongs to actionlint and the separate YAML check; these tests exercise
install order and the actual image gate expressions against job outcomes.
"""

import ast
import json
from pathlib import Path
import re
import unittest


WORKFLOW = Path(__file__).parent / "workflows" / "ci.yml"


def job_blocks(source):
    headers = list(re.finditer(r"^  ([a-z][a-z0-9-]*):\s*$", source, re.M))
    return {
        header[1]: source[header.end():headers[index + 1].start()
                          if index + 1 < len(headers) else len(source)]
        for index, header in enumerate(headers)
    }


def job_condition(block):
    match = re.search(r"^    if: (.+?)(?=\n    [a-z]|\Z)", block, re.M | re.S)
    if not match:
        raise AssertionError("job needs an explicit condition")
    value = match[1].strip()
    if value.startswith(">-"):
        value = value[2:].strip()
    if not (value.startswith("${{") and value.endswith("}}")):
        raise AssertionError("expected a GitHub expression")
    return value[3:-2].strip()


def evaluate_condition(expression, context, cancelled=False):
    """Interpret the limited boolean grammar used by image jobs; never eval."""
    expression = expression.replace("&&", " and ").replace("||", " or ")
    expression = re.sub(r"!(?!=)", " not ", expression)
    tree = ast.parse(" ".join(expression.split()), mode="eval")

    def visit(node):
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bool)):
            return node.value
        if isinstance(node, ast.Name):
            return context[node.id]
        if isinstance(node, ast.Attribute):
            return visit(node.value)[node.attr]
        if isinstance(node, ast.Subscript):
            return visit(node.value)[visit(node.slice)]
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            return not visit(node.operand)
        if isinstance(node, ast.BoolOp):
            values = [bool(visit(value)) for value in node.values]
            if isinstance(node.op, ast.And):
                return all(values)
            if isinstance(node.op, ast.Or):
                return any(values)
        if isinstance(node, ast.Compare) and len(node.ops) == 1:
            left, right = visit(node.left), visit(node.comparators[0])
            if isinstance(node.ops[0], ast.Eq):
                return left == right
            if isinstance(node.ops[0], ast.NotEq):
                return left != right
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                and node.func.id == "cancelled" and not node.args
                and not node.keywords):
            return cancelled
        raise AssertionError(f"unsupported expression node: {ast.dump(node)}")

    return bool(visit(tree.body))


def needs(block):
    match = re.search(r"^    needs: \[([^\]]+)\]$", block, re.M)
    if not match:
        raise AssertionError("expected an explicit needs list")
    return [value.strip() for value in match[1].split(",")]


class TransportWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = WORKFLOW.read_text()
        cls.jobs = job_blocks(cls.source)

    def job(self, name):
        self.assertTrue(name in self.jobs, f"missing {name} job")
        return self.jobs[name]

    def assert_node_pins(self, block):
        self.assertRegex(block, r'node-version: [\'"]22\.23\.3[\'"]')
        self.assertRegex(block, r"npm install (?:--global|-g) npm@12\.0\.2(?:\s|$)")

    def assert_consumer_install(self, job, directory, consumer_command):
        block = self.job(job)
        self.assert_node_pins(block)
        transport = re.search(r"run: npm ci --prefix transport/js --ignore-scripts(?:\s|$)", block)
        self.assertIsNotNone(transport, "install transport's lock without lifecycle scripts first")
        consumer = re.search(
            rf"working-directory: {directory}\n\s+run: {re.escape(consumer_command)}(?:\s|$)",
            block,
        )
        self.assertIsNotNone(consumer, "install the consumer's committed lock")
        self.assertLess(transport.start(), consumer.start())

    def test_filter_exports_transport(self):
        self.assertIn("transport: ${{ steps.filter.outputs.transport }}", self.job("changes"))

    def test_no_external_transport_checkout_or_python_override(self):
        self.assertNotRegex(self.source, r"git clone[^\n]*build-secure-transport")
        self.assertNotIn("../build-secure-transport", self.source)
        self.assertNotIn("BUILD_SECURE_TRANSPORT_PY:", self.source)
        self.assertNotIn("/tmp/bst", self.source)

    def test_bridge_uv_pin_precedes_test(self):
        block = self.job("bridge")
        self.assertRegex(block, r'version: [\'"]0\.12\.13[\'"]')
        self.assertLess(block.index("Install uv"), block.index("run: cargo test --all"))

    def test_transport_job_is_gated_and_runs_every_binding(self):
        block = self.job("transport")
        self.assertEqual(needs(block), ["changes"])
        expression = job_condition(block)
        for owed in ("true", "false"):
            with self.subTest(owed=owed):
                context = {"needs": {"changes": {"outputs": {"transport": owed}}}}
                self.assertEqual(evaluate_condition(expression, context), owed == "true")
        self.assertRegex(block, r'version: [\'"]0\.12\.13[\'"]')
        self.assert_node_pins(block)
        self.assertIn("npm ci --prefix transport/js --ignore-scripts", block)
        self.assertRegex(block, r"python -m unittest discover -s (?:transport/)?python/tests(?:\s|$)")
        self.assertRegex(block, r"python -m unittest discover -s (?:transport/)?compatibility(?:\s|$)")
        self.assertRegex(block, r"npm test --prefix transport/js(?:\s|$)")
        self.assertRegex(block, r"cargo test(?: --locked)? --lib transport(?:\s|$)")
        self.assertRegex(block, r"cargo test(?: --locked)? --test transport_vectors(?:\s|$)")
        self.assertRegex(block, r"cargo test(?: --locked)? --test interop_python(?:\s|$)")
        self.assertIn("--frozen", block)

    def test_existing_bridge_interop_remains(self):
        block = self.job("interop")
        self.assertIn("needs.changes.outputs.bridge == 'true'", job_condition(block))
        self.assertIn("cargo test --test interop_python -- --nocapture", block)

    def test_spa_install_order_and_tests(self):
        self.assert_consumer_install("spa", "spa", "npm ci --legacy-peer-deps")
        block = self.job("spa")
        for command in ("npm run lint", "npx vitest run", "npm run test:captures"):
            self.assertIn(command, block)

    def test_web_install_order_and_tests(self):
        self.assert_consumer_install("web", "web", "npm ci")
        block = self.job("web")
        expression = job_condition(block)
        for owed in ("true", "false"):
            context = {"needs": {"changes": {"outputs": {"e2e": owed}}}}
            self.assertEqual(evaluate_condition(expression, context), owed == "true")
        self.assertRegex(block, r"working-directory: web\n\s+run: npm test(?:\s|$)")
        self.assertIn("web", needs(self.job("build-relay")))
        self.assertIn("needs.web.result == 'success'", job_condition(self.job("build-relay")))

    def test_web_requires_bridge_extension_tests_and_locked_typecheck(self):
        block = self.job("web")
        directory = "bridge/tests/fixtures"
        install = f"nice -n 10 npm ci --prefix {directory} --ignore-scripts"
        self.assertIn(install, block)
        self.assertNotIn("continue-on-error", block)
        self.assertNotRegex(block, r"^    defaults:", "these commands run from the repository root")
        package = json.loads((WORKFLOW.parents[2] / directory / "package.json").read_text())
        lock = json.loads((WORKFLOW.parents[2] / directory / "package-lock.json").read_text())
        for script in ("test:compaction", "typecheck:compaction"):
            command = f"nice -n 10 npm run --prefix {directory} {script}"
            self.assertIn(command, block)
            self.assertLess(block.index(install), block.index(command))
            self.assertNotRegex(package["scripts"][script], r"[;&|]", "a failed checker must fail its CI step")
        for dependency, version in package["devDependencies"].items():
            self.assertRegex(version, r"^\d+\.\d+\.\d+$", "checker dependencies must be pinned")
            self.assertEqual(lock["packages"][f"node_modules/{dependency}"]["version"], version)
        self.assertEqual(package["scripts"]["test:compaction"], "node --test pi-compaction-extension.test.mjs")
        self.assertEqual(package["scripts"]["typecheck:compaction"], "node check-pi-compaction-types.mjs node_modules/typescript/bin/tsc node_modules/@types")

    def image_context(self, block, owed, result):
        return {
            "github": {"event_name": "push", "ref": "refs/heads/main"},
            "needs": {
                **{name: {"result": "success"} for name in needs(block)},
                "changes": {"outputs": {"app": "true", "relay": "true",
                                         "bridge": "false", "transport": owed}},
                "transport": {"result": result},
            },
        }

    def test_actual_image_gates_reject_failed_cancelled_and_skipped_transport(self):
        for name in ("build-app", "build-relay"):
            block = self.job(name)
            self.assertIn("transport", needs(block))
            expression = job_condition(block)
            for result in ("success", "failure", "cancelled", "skipped", ""):
                with self.subTest(job=name, owed=True, result=result):
                    context = self.image_context(block, "true", result)
                    self.assertEqual(evaluate_condition(expression, context), result == "success")
            for result in ("success", "skipped"):
                with self.subTest(job=name, owed=False, result=result):
                    context = self.image_context(block, "false", result)
                    self.assertTrue(evaluate_condition(expression, context))
            with self.subTest(job=name, workflow_cancelled=True):
                context = self.image_context(block, "true", "success")
                self.assertFalse(evaluate_condition(expression, context, cancelled=True))

    def test_app_keeps_owed_bridge_and_interop_guard(self):
        block = self.job("build-app")
        expression = job_condition(block)
        for job in ("bridge", "bridge-macos", "interop"):
            self.assertIn(job, needs(block))
            for result in ("failure", "cancelled", "skipped"):
                with self.subTest(job=job, result=result):
                    context = self.image_context(block, "true", "success")
                    context["needs"]["changes"]["outputs"]["bridge"] = "true"
                    context["needs"][job]["result"] = result
                    self.assertFalse(evaluate_condition(expression, context))
        context = self.image_context(block, "false", "skipped")
        for job in ("bridge", "bridge-macos", "interop"):
            context["needs"][job]["result"] = "skipped"
        self.assertTrue(evaluate_condition(expression, context))


if __name__ == "__main__":
    unittest.main(verbosity=2)
