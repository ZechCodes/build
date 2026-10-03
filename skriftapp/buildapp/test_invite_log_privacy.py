"""Error logging must not turn an invite link into a retained credential."""
import logging


def test_framework_and_server_logs_redact_invite_paths_and_exception_text(caplog):
    import buildapp.invites_controller  # noqa: F401 -- installs route logging privacy

    token = "inv_" + "secret" * 7
    for logger_name in ("skrift.lib.exceptions", "hypercorn.error", "hypercorn.access"):
        with caplog.at_level(logging.ERROR, logger=logger_name):
            try:
                raise ValueError(f"bad link /invite/{token}")
            except ValueError:
                logging.getLogger(logger_name).exception(
                    "Unhandled exception on %s %s", "GET", f"/invite/{token}"
                )
        assert token not in caplog.text
        assert "[redacted]" in caplog.text
        caplog.clear()
