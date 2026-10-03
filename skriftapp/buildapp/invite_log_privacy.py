"""Keep invite credentials out of the framework's error and server access logs.

Skrift logs the request path on an unexpected exception. The route contains a
credential, so even failed requests must redact it, including exception text.
The shipped server disables access logs; the same filter protects them if enabled.
"""

import logging
import re

_TOKEN = re.compile(r"inv_[A-Za-z0-9_-]+")
_LINK = re.compile(r"(/invite/)[^\s/?#\"'<>]+")


def _redact(text: str) -> str:
    return _TOKEN.sub("[redacted]", _LINK.sub(r"\1[redacted]", text))


class InviteTokenFilter(logging.Filter):
    def filter(self, record: logging.LogRecord) -> bool:
        record.msg = _redact(record.getMessage())
        record.args = ()
        if record.exc_info:
            record.exc_text = _redact(logging.Formatter().formatException(record.exc_info))
            record.exc_info = None
        if record.stack_info:
            record.stack_info = _redact(record.stack_info)
        return True


def install_invite_log_redaction() -> None:
    """Install once when the invite routes load, before they can receive requests."""
    for name in ("skrift.lib.exceptions", "hypercorn.error", "hypercorn.access"):
        logger = logging.getLogger(name)
        if not any(isinstance(item, InviteTokenFilter) for item in logger.filters):
            logger.addFilter(InviteTokenFilter())
