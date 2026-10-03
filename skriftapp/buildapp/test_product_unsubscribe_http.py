"""A member can withdraw product consent with the existing signed email link."""
from litestar.testing import TestClient
from skrift.config import get_settings
from sqlalchemy import select

from buildapp.clock import utc_now
from buildapp.db_test_support import add_account
from buildapp.email_consent import consenting_product_email_addresses, record_signup_consent
from buildapp.models import WaitlistSignup
from buildapp.skrift_app_test_support import SECURE_ORIGIN, on_database
from buildapp.waitlist_unsubscribe_token import mint_unsubscribe_token, unsubscribe_path


def test_product_unsubscribe_is_read_only_on_get_and_repeatable_on_post(skrift_app):
    async def seed(session):
        for email in ("waitlisted@example.com", "member-only@example.com"):
            user_id = await add_account(session, email)
            record_signup_consent(session, user_id, True, utc_now())
        session.add(WaitlistSignup(email="waitlisted@example.com"))
        await session.commit()

    async def waitlist_addresses(session):
        return list((await session.execute(select(WaitlistSignup.email))).scalars())

    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        on_database(client, seed)
        for email in ("waitlisted@example.com", "member-only@example.com"):
            path = unsubscribe_path(mint_unsubscribe_token(email, get_settings().secret_key))
            assert client.get(path).status_code == 200
            assert email in on_database(client, consenting_product_email_addresses)
            assert client.post(path + "invalid").status_code == 404
            assert email in on_database(client, consenting_product_email_addresses)
            assert client.post(path).status_code == 200
            assert client.post(path).status_code == 200
            assert email not in on_database(client, consenting_product_email_addresses)
        assert on_database(client, waitlist_addresses) == []
