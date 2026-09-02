"""Email backends for tests: one that records every send and one that always fails."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class SentEmail:
    to: str
    subject: str
    text_body: str
    html_body: str | None
    headers: dict[str, str] | None


class EmailDeliveryFailure(Exception):
    pass


class RecordingEmailBackend:
    def __init__(self) -> None:
        self.sent: list[SentEmail] = []

    async def start(self) -> None:
        return None

    async def stop(self) -> None:
        return None

    async def send_email(
        self,
        to: str,
        subject: str,
        text_body: str,
        *,
        html_body: str | None = None,
        from_address: str | None = None,
        reply_to: str | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        self.sent.append(
            SentEmail(
                to=to,
                subject=subject,
                text_body=text_body,
                html_body=html_body,
                headers=headers,
            )
        )


class FailingEmailBackend:
    async def start(self) -> None:
        return None

    async def stop(self) -> None:
        return None

    async def send_email(
        self,
        to: str,
        subject: str,
        text_body: str,
        *,
        html_body: str | None = None,
        from_address: str | None = None,
        reply_to: str | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        raise EmailDeliveryFailure(f"delivery to {to} failed")
