"""Qualify real bridge discovery proofs against the disposable hosted issuer."""

import argparse
import asyncio
from datetime import timedelta
import json
import os
from pathlib import Path
import ssl
import sys
from urllib.parse import urlsplit
from uuid import UUID

SCRATCH = Path("/home/zech/Projects/.agent-scratch/976")


class DiscoveryQualification:
    def __init__(self, fixture, client, maker):
        self.fixture = fixture
        self.client = client
        self.maker = maker
        self.evidence = []

    async def credential_count(self):
        from sqlalchemy import func, select
        from buildapp.models import BridgeHttpToken

        async with self.maker() as session:
            return await session.scalar(select(func.count(BridgeHttpToken.id)))

    def challenge(self, origin=None):
        response = self.client.post(self.fixture["hostedOrigin"] + "/api/bridge-discovery/challenges",
            json={"origin": origin or self.fixture["bridgeOrigin"]})
        assert response.status_code == 201, f"Challenge allocation HTTP {response.status_code}"
        return response.json()

    def proof(self, challenge):
        response = self.client.post(self.fixture["bridgeOrigin"] + "/.well-known/build-bridge",
            headers={"Authorization": "", "Origin": self.fixture["hostedOrigin"]},
            json={"challenge_id": challenge["challenge_id"], "nonce": challenge["nonce"]})
        assert response.status_code == 200, f"Bridge proof route HTTP {response.status_code}"
        proof = response.json().get("proof")
        assert isinstance(proof, str) and len(proof.split(".")) == 3
        return proof

    async def complete(self, case, challenge, proof, *, accepted=False):
        before = await self.credential_count()
        response = self.client.post(self.fixture["hostedOrigin"] + "/api/bridge-discovery/complete",
            json={"challenge_id": challenge["challenge_id"], "proof": proof})
        after = await self.credential_count()
        body = response.json()
        expected_status = 201 if accepted else 401
        assert response.status_code == expected_status, f"{case}: HTTP {response.status_code}"
        assert after - before == int(accepted), f"{case}: credential row delta {after - before}"
        assert ("access_token" in body) is accepted, f"{case}: unexpected credential response"
        if accepted:
            assert body["account_id"] == self.fixture["accountId"]
            assert body["device_id"] == self.fixture["deviceId"]
            assert body["origin"] == self.fixture["bridgeOrigin"]
        record = {"case": case, "proof_route_status": 200, "completion_status": response.status_code,
            "credential_count_before": before, "credential_count_after": after,
            "credential_count_delta": after - before, "credential_returned": "access_token" in body}
        self.evidence.append(record)
        print(json.dumps(record, sort_keys=True), flush=True)

    async def expire_challenge(self, challenge):
        from sqlalchemy import select
        from buildapp.clock import utc_now
        from buildapp.models import BridgeHttpChallenge

        async with self.maker() as session:
            row = await session.scalar(select(BridgeHttpChallenge).where(
                BridgeHttpChallenge.id == UUID(challenge["challenge_id"]),
                BridgeHttpChallenge.user_id == UUID(self.fixture["accountId"]),
                BridgeHttpChallenge.purpose == "discovery"))
            assert row is not None and row.consumed_at is None
            row.expires_at = utc_now() - timedelta(seconds=1)
            await session.commit()

    async def run(self):
        positive = self.challenge()
        await self.complete("valid_route_proof", positive, self.proof(positive), accepted=True)

        tampered = self.challenge()
        parts = self.proof(tampered).split(".")
        parts[2] = ("A" if parts[2][0] != "A" else "B") + parts[2][1:]
        await self.complete("tampered_signature", tampered, ".".join(parts))

        port = urlsplit(self.fixture["bridgeOrigin"]).port
        wrong = self.challenge(f"https://localhost:{19477 if port != 19477 else 19478}")
        await self.complete("wrong_pending_origin", wrong, self.proof(wrong))

        expired = self.challenge()
        proof = self.proof(expired)
        await self.expire_challenge(expired)
        await self.complete("expired_challenge", expired, proof)

        reused = self.challenge()
        proof = self.proof(reused)
        await self.complete("reused_challenge_first_use", reused, proof, accepted=True)
        await self.complete("reused_challenge_second_use", reused, proof)


def load_fixture(path, scratch=SCRATCH):
    fixture = json.loads(path.read_text())
    run_dir = Path(fixture["runDir"]).resolve(strict=True)
    if not run_dir.is_relative_to(scratch.resolve()) or not run_dir.name.startswith("run-"):
        raise ValueError("Discovery qualification may alter TTL only in a disposable task run directory")
    for name in ["hostedOrigin", "bridgeOrigin"]:
        parsed = urlsplit(fixture[name])
        if parsed.scheme != "https" or parsed.hostname != "localhost" or not parsed.port:
            raise ValueError("Discovery qualification requires explicit localhost HTTPS ports")
    database = run_dir / "app.db"
    if not database.is_file() or not database.resolve().is_relative_to(run_dir):
        raise ValueError("Disposable SQLite database is missing or escapes its run directory")
    return fixture, run_dir


async def qualify(fixture, run_dir):
    import httpx
    from buildapp.db_test_support import file_session_maker

    context = ssl.create_default_context(cafile=str(run_dir / "ca.pem"))
    maker = file_session_maker(run_dir / "app.db")
    try:
        with httpx.Client(verify=context, follow_redirects=False, timeout=5, trust_env=False,
                headers={"Authorization": "Bearer " + fixture["accessToken"]}) as client:
            qualification = DiscoveryQualification(fixture, client, maker)
            await qualification.run()
            (run_dir / "discovery-evidence.json").write_text(json.dumps(qualification.evidence, indent=2) + "\n")
    finally:
        await maker.kw["bind"].dispose()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, default=os.environ.get("BUILD_HTTP_QUALIFICATION_FIXTURE"))
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--scratch-task", type=int, choices=[976, 977], default=976)
    args = parser.parse_args()
    if args.fixture is None:
        parser.error("--fixture or BUILD_HTTP_QUALIFICATION_FIXTURE is required")
    scratch = Path("/home/zech/Projects/.agent-scratch") / str(args.scratch_task)
    fixture, run_dir = load_fixture(Path(args.fixture), scratch)
    sys.path.insert(0, str(args.repo.resolve() / "skriftapp"))
    os.chdir(run_dir)
    os.environ.update(SKRIFT_ENV="dev", HOME=str(run_dir / "home"), TMPDIR=str(run_dir / "tmp"))
    asyncio.run(qualify(fixture, run_dir))


if __name__ == "__main__":
    main()
