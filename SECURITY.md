# Security policy

## Supported versions

Security fixes land on the latest bridge release and `main`. Older releases
are not patched; update the bridge to the latest release.

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's private vulnerability
reporting: open [Build’s private reporting form](https://github.com/ZechCodes/build/security/advisories/new)
(or this repository’s **Security** tab → **Report a vulnerability**). Please do not open a public issue or pull request for a
security problem.

Include what you found, how to reproduce it, and the version or commit you
tested against. You will get a reply on the report.

## End-to-end encryption transport

Report transport vulnerabilities through the same private Build reporting
path above, including those in `transport/js`, `transport/python` or
`bridge/src/transport.rs`. Build owns their maintenance and audit history;
[UPSTREAM.md](transport/UPSTREAM.md) preserves the original repository pin and
import hashes. The legacy repository remains available for provenance and
rollback; it is no longer the maintenance source of truth.
