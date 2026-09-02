"""Test doubles and fixtures for outbound mail: one backend that records every send, one
that always fails, and the waitlist email context both are exercised through."""

from __future__ import annotations

from dataclasses import dataclass

from skrift.config import AuthConfig, EmailConfig, Settings

from buildapp.waitlist_mail import WaitlistEmailContext
from buildapp.waitlist_unsubscribe_token import unsubscribe_url

PUBLIC_BASE_URL = "https://getbuild.ing"
SECRET_KEY = "the-signing-key"
OWNER_ADDRESS = "hi@zech.sh"
FIXTURE_TOKEN = "token-value"
UNSUBSCRIBE_URL = unsubscribe_url(PUBLIC_BASE_URL, FIXTURE_TOKEN)


def waitlist_email_context(*, notify_address: str = "") -> WaitlistEmailContext:
    return WaitlistEmailContext(
        public_base_url=PUBLIC_BASE_URL,
        secret_key=SECRET_KEY,
        notify_address=notify_address,
    )


def email_settings(
    *,
    public_base_url: str = PUBLIC_BASE_URL,
    redirect_base_url: str = "",
    secret_key: str = SECRET_KEY,
) -> Settings:
    return Settings(
        secret_key=secret_key,
        email=EmailConfig(public_base_url=public_base_url),
        auth=AuthConfig(redirect_base_url=redirect_base_url),
    )


@dataclass(frozen=True)
class SentEmail:
    to: str
    subject: str
    text_body: str
    html_body: str | None
    headers: dict[str, str] | None


class EmailDeliveryFailure(Exception):
    pass


class StubEmailBackend:
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
        self.record(
            SentEmail(
                to=to,
                subject=subject,
                text_body=text_body,
                html_body=html_body,
                headers=headers,
            )
        )

    def record(self, email: SentEmail) -> None:
        raise NotImplementedError


class RecordingEmailBackend(StubEmailBackend):
    def __init__(self) -> None:
        self.sent: list[SentEmail] = []

    def record(self, email: SentEmail) -> None:
        self.sent.append(email)


class FailingEmailBackend(StubEmailBackend):
    def record(self, email: SentEmail) -> None:
        raise EmailDeliveryFailure(f"delivery to {email.to} failed")
