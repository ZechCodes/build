# Secure transport provenance

Source: `ZechCodes/build-secure-transport` at `4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`.
Import only regular tracked blobs through `scripts/transport_import.py import`.
Working-tree bytes, symlinks and directories are never copied from the sibling.

## Explicit import allowlist and SHA-256

Each upstream path maps to `transport/<path>`. Source SHA is the unmodified
pinned blob; current SHA records separately reviewed metadata/doc amendments.
Executable sources, baseline tests and LICENSE.md must keep the source digest.

| Upstream path | Source SHA-256 | Current SHA-256 |
| --- | --- | --- |
| `E2E-MVP.md` | `984ddc9c7582a6340b073274602d03f3e50a889d2ae8deb42de9f3b7d4e35c38` | `984ddc9c7582a6340b073274602d03f3e50a889d2ae8deb42de9f3b7d4e35c38` |
| `LICENSE.md` | `964cd7fcc4be51375162c2bbcf1b814e264d6477a24469530e73e1b113e616f2` | `964cd7fcc4be51375162c2bbcf1b814e264d6477a24469530e73e1b113e616f2` |
| `README.md` | `4794e30bbf5b744b5b05f1186cdd804185cc6fc673f6ec3d8fd4804f39e6d54e` | `4794e30bbf5b744b5b05f1186cdd804185cc6fc673f6ec3d8fd4804f39e6d54e` |
| `js/README.md` | `5cc959b5d8fe9a31cb273a18eca62840debb441b4294680c0a878c955e0a75b1` | `5cc959b5d8fe9a31cb273a18eca62840debb441b4294680c0a878c955e0a75b1` |
| `js/package-lock.json` | `fe06ddb52117f0767fba5e613167a446a13879ff07bfd5417a4c317dd3ebba5f` | `fe06ddb52117f0767fba5e613167a446a13879ff07bfd5417a4c317dd3ebba5f` |
| `js/package.json` | `17ad3fb3cf0874c4c640295620113ad167741ce1d9a910f5d7519492c85a4208` | `17ad3fb3cf0874c4c640295620113ad167741ce1d9a910f5d7519492c85a4208` |
| `js/src/index.js` | `55f85d864691aa4915a32876ff1511af2d24083adeba744d9935a834b64dc443` | `55f85d864691aa4915a32876ff1511af2d24083adeba744d9935a834b64dc443` |
| `js/test/index.test.js` | `5dfd2c9fbe0a6eec6edb625821a2c350513e0e940d6c5177d2bb6404b8954ef4` | `5dfd2c9fbe0a6eec6edb625821a2c350513e0e940d6c5177d2bb6404b8954ef4` |
| `python/README.md` | `d7f6e12fd9feb8e7472c1cd013c91eedbe3a894401309eee407689ece0d9e8fd` | `d7f6e12fd9feb8e7472c1cd013c91eedbe3a894401309eee407689ece0d9e8fd` |
| `python/pyproject.toml` | `e7ad92d3d1bcf82ee7176b9d9a23568be303fdb5d1fa9e68ae26f70082e888a6` | `e7ad92d3d1bcf82ee7176b9d9a23568be303fdb5d1fa9e68ae26f70082e888a6` |
| `python/src/build_secure_transport/__init__.py` | `0cc488cf4f82833d6fe7547773eed70f3f9b1281b73f155b70141ac7817c3a19` | `0cc488cf4f82833d6fe7547773eed70f3f9b1281b73f155b70141ac7817c3a19` |
| `python/src/build_secure_transport/_protocol.py` | `9e0e177ac186a41f65c2169ea328b94a397a46a1f0c1359ce2110666dc7015ed` | `9e0e177ac186a41f65c2169ea328b94a397a46a1f0c1359ce2110666dc7015ed` |
| `python/src/build_secure_transport/device.py` | `6bfef20825ee35e7294ed80d53b944c14869724f39d89905a8418c8beada635b` | `6bfef20825ee35e7294ed80d53b944c14869724f39d89905a8418c8beada635b` |
| `python/src/build_secure_transport/exceptions.py` | `2ca01dce41552dc196c65f76f488f7a8f748d9f8419fd26b156a31f154c63548` | `2ca01dce41552dc196c65f76f488f7a8f748d9f8419fd26b156a31f154c63548` |
| `python/tests/interop_helper.py` | `828b973fe961fd1d07fb77935407df8d75f51874d2e6440bbb4308501f668b8b` | `828b973fe961fd1d07fb77935407df8d75f51874d2e6440bbb4308501f668b8b` |
| `python/tests/test_device.py` | `27a275599f0888af302667848df76d5e6ee2b712ab357acf6c110740d66dd24c` | `27a275599f0888af302667848df76d5e6ee2b712ab357acf6c110740d66dd24c` |
| `python/uv.lock` | `800e16b959c20ba90bd4341f44bf1ef3671233c9edab2fee8584f21f83cec97d` | `800e16b959c20ba90bd4341f44bf1ef3671233c9edab2fee8584f21f83cec97d` |

## Exclusions

Tracked files omitted at the pinned commit:

- `.gitignore`
- `python/build/lib/build_secure_transport/__init__.py`
- `python/build/lib/build_secure_transport/_protocol.py`
- `python/build/lib/build_secure_transport/device.py`
- `python/build/lib/build_secure_transport/exceptions.py`
- `python/src/build_secure_transport.egg-info/PKG-INFO`
- `python/src/build_secure_transport.egg-info/SOURCES.txt`
- `python/src/build_secure_transport.egg-info/dependency_links.txt`
- `python/src/build_secure_transport.egg-info/requires.txt`
- `python/src/build_secure_transport.egg-info/top_level.txt`

Also excluded regardless of checkout state: `.git/`, `.rift/`, `node_modules/`,
`.venv/`, caches, build artifacts and the untracked recursive
`build-secure-transport` symlink. Build's existing root ignores are retained.

## Amendments

None in the initial import. The initial import commit preserves all listed bytes.
