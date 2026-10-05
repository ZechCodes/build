"""Fixture entry points reject host-root execution before any network effects."""

import unittest
from unittest.mock import AsyncMock, MagicMock, patch

import network
import router
import run as runner
from isolation import require_private_namespace


class NamespaceGuardTests(unittest.TestCase):
    def test_accepts_mapped_root_with_one_uid(self):
        with (
            patch("pathlib.Path.read_text", return_value="0 1000 1\n"),
            patch("os.geteuid", return_value=0),
        ):
            require_private_namespace()

    def test_rejects_nonroot_and_host_or_multiple_uid_ranges(self):
        for mapping, effective_uid in [
            ("0 0 4294967295\n", 0),
            ("0 1000 1\n", 1000),
            ("0 1000 1\n1 1001 1\n", 0),
            ("", 0),
        ]:
            with (
                self.subTest(mapping=mapping, effective_uid=effective_uid),
                patch("pathlib.Path.read_text", return_value=mapping),
                patch("os.geteuid", return_value=effective_uid),
            ):
                with self.assertRaisesRegex(AssertionError, "disposable single-user namespace"):
                    require_private_namespace()


class NamespaceEntryTests(unittest.IsolatedAsyncioTestCase):
    async def test_router_rejects_host_root_before_listener_or_socket_changes(self):
        artifacts = MagicMock()
        with (
            patch("pathlib.Path.read_text", return_value="0 0 4294967295\n"),
            patch("os.geteuid", return_value=0),
            patch.object(router.asyncio, "get_running_loop") as get_loop,
            patch.object(router.asyncio, "start_server") as listen,
        ):
            with self.assertRaisesRegex(AssertionError, "disposable single-user namespace"):
                await router.main(artifacts)
            get_loop.assert_not_called()
            listen.assert_not_called()
            self.assertEqual(artifacts.mock_calls, [])

    async def test_mdns_listener_binds_the_multicast_group(self):
        loop = MagicMock()
        loop.create_datagram_endpoint = AsyncMock(side_effect=[
            (MagicMock(), MagicMock()),
            RuntimeError("stop before starting the fixture"),
        ])
        mdns_socket = MagicMock()
        with (
            patch("pathlib.Path.read_text", return_value="0 1000 1\n"),
            patch("os.geteuid", return_value=0),
            patch.object(network.asyncio, "get_running_loop", return_value=loop),
            patch.object(network.socket, "socket", return_value=mdns_socket),
            patch.object(network.subprocess, "run") as nft,
        ):
            with self.assertRaisesRegex(RuntimeError, "stop before starting the fixture"):
                await network.main(MagicMock())
            mdns_socket.bind.assert_called_once_with(("224.0.0.251", 5353))
            nft.assert_not_called()

    async def test_network_rejects_host_root_before_socket_or_firewall_changes(self):
        loop = MagicMock()
        loop.create_datagram_endpoint = AsyncMock(
            side_effect=AssertionError("network setup happened before the namespace guard")
        )
        artifacts = MagicMock()
        with (
            patch("pathlib.Path.read_text", return_value="0 0 4294967295\n"),
            patch("os.geteuid", return_value=0),
            patch.object(network.asyncio, "get_running_loop", return_value=loop) as get_loop,
            patch.object(network.socket, "socket") as open_socket,
            patch.object(network.subprocess, "run") as nft,
        ):
            with self.assertRaisesRegex(AssertionError, "disposable single-user namespace"):
                await network.main(artifacts)
            get_loop.assert_not_called()
            open_socket.assert_not_called()
            nft.assert_not_called()
            self.assertEqual(artifacts.mock_calls, [])

    def test_runner_rejects_host_root_before_process_or_firewall_changes(self):
        artifacts = MagicMock()
        with (
            patch("pathlib.Path.read_text", return_value="0 0 4294967295\n"),
            patch("os.geteuid", return_value=0),
            patch.object(runner.subprocess, "run") as nft,
            patch.object(runner.subprocess, "Popen") as start_process,
        ):
            with self.assertRaisesRegex(AssertionError, "disposable single-user namespace"):
                runner.inside(MagicMock(), artifacts)
            self.assertEqual(artifacts.mock_calls, [])
            nft.assert_not_called()
            start_process.assert_not_called()


if __name__ == "__main__":
    unittest.main()
