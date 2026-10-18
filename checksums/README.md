# Reviewed release pins

`scripts/install.sh` trusts `v<version>.sha256` in this source checkout before
executing, moving or linking a downloaded binary. Each row has three fields:
release tag, lowercase SHA256, exact asset filename. All rows must match the
requested version; malformed rows, duplicate assets and a missing platform pin
cause a local `cargo build --release --locked` instead. Download mirrors use
the same pins. Remote `SHA256SUMS` is not downloaded or trusted by the installer.

For new releases, follow [Releasing](../AGENTS.md#releasing): verify the built
artifacts independently, then review and commit pins in a follow-up change.
A source checkout without pins for its version builds from source.

## v0.2.1 verification

All four assets were freshly downloaded from
[the release](https://github.com/zyrre/corgi/releases/tag/v0.2.1).
SHA256 of their bytes matches every entry in `v0.2.1.sha256`, the published
`SHA256SUMS`, and each GitHub API asset digest. Each binary is also byte-for-byte
identical to the retained original publication download under
`/tmp/corgi-release-0.2.1-publish/assets/`, cross-checking its `REPORT.md`.
The two mutable remote checksum sources alone would not establish these pins.

| Asset | Verified bytes |
| --- | ---: |
| corgi-aarch64-apple-darwin | 4400144 |
| corgi-aarch64-unknown-linux-musl | 4396184 |
| corgi-x86_64-apple-darwin | 4653328 |
| corgi-x86_64-unknown-linux-musl | 5289664 |

The native x86_64 Linux asset installed through the updated script from the
public release in a scratch checkout, with isolated HOME/config, `CORGI_LINK=0`
and a failing Cargo stub to exclude source fallback. Installed bytes match the
verified download and report `corgi 0.2.1`.

`bash scripts/test-install` includes an attacker executable with a side-effect
marker and a matching replacement remote `SHA256SUMS`. The updated installer
rejects it without executing it and builds local source. Running this test
against the previous installer creates the marker and installs the replacement.
Invalid-pin tests start with pins for the exact marker-bearing bytes and then
invalidate only the trusted data, so a binary mismatch cannot mask a parser bug.
The suite also covers trusted downloads, altered bytes, absent or malformed
remote sums, source fallback and linking. ShellCheck passes for all three
installer/link/test scripts. No Rust code changed.

This source change requires review, merge and publication through the normal
source installation path before it remediates public installs. Existing release
tags and already installed checkouts retain their previous installer behavior.
No release, marketplace update or installed plugin change is part of this work.

## v0.2.2 verification

The [release](https://github.com/zyrre/corgi/releases/tag/v0.2.2) was published by
[run 39132688924, attempt 1](https://github.com/zyrre/corgi/actions/runs/39132688924)
from merged commit
[`4a0e1aa11758c55ec72052c667596281430b82a7`](https://github.com/zyrre/corgi/commit/4a0e1aa11758c55ec72052c667596281430b82a7).
The version gate, all four builds, release publication and actual Omarchy
publication succeeded. All four binaries were freshly downloaded from the
public release and independently hashed. Each matches `v0.2.2.sha256`, the
published `SHA256SUMS`, its GitHub release asset digest, and the bytes extracted
from its separately downloaded CI artifact below. Each retained artifact archive
also matches its GitHub artifact digest and identifies this run and source commit.
Run/job metadata, logs, artifact archives and release downloads were retained
for review; mutable release sums and API hashes alone did not establish the pins.

| Asset | Verified bytes | CI artifact ID |
| --- | ---: | ---: |
| corgi-aarch64-apple-darwin | 4417472 | 12138023786 |
| corgi-x86_64-apple-darwin | 4666496 | 12138611359 |
| corgi-aarch64-unknown-linux-musl | 4396248 | 12138562540 |
| corgi-x86_64-unknown-linux-musl | 5265152 | 12139065368 |

The native x86_64 Linux public binary is static and reports `corgi 0.2.2`.
Isolated CLI checks under umask 022 verified private creation, legacy and orphan
repair, delivery, compaction and report content preservation, with shared parents
unchanged. Candidate installation uses the real public download, isolated
HOME/config, `CORGI_LINK=0` and a failing Cargo stub to exclude source fallback;
installed bytes match the verified asset. Installer regression tests and
ShellCheck pass. Other platforms have CI coverage: arm64 macOS and both Linux
assets run there; x86_64 macOS is cross-built without a runtime smoke test.

The published [Omarchy commit `f92e3b7320afeb8c78c36a0f876b4be416ddc572`](https://github.com/zyrre/omarchy-corgi/commit/f92e3b7320afeb8c78c36a0f876b4be416ddc572)
matches the tag's `omarchy/` plus `LICENSE` by paths, modes, blobs and bytes,
retains the previous public documentation history, and passes both manifest
validators. This does not activate an installed widget.

These pins are a separate candidate for user review and merge. Until reviewed
pins reach the public source installation path, v0.2.2 installs build from source.
The release tag itself has no v0.2.2 pins; candidate installation does not verify
the later public source installation or local activation steps.

## v0.2.3 verification

The [release](https://github.com/zyrre/corgi/releases/tag/v0.2.3) was published by
[run 39160739431, attempt 1](https://github.com/zyrre/corgi/actions/runs/39160739431)
from merged commit
[`ba6ee6c2cfd2b6426b907fc398964592200b3c09`](https://github.com/zyrre/corgi/commit/ba6ee6c2cfd2b6426b907fc398964592200b3c09).
The version gate, all four builds, release publication and actual Omarchy
publication succeeded. All four binaries were freshly downloaded and hashed.
Each matches `v0.2.3.sha256`, the published `SHA256SUMS`, its GitHub asset
digest, publication log and bytes from its separately downloaded CI artifact.
Each retained artifact archive matches its API digest and identifies this exact
run and source commit. Raw downloads, archives, metadata and publication logs
were retained for review; mutable release sums/API hashes alone did not
establish these pins. Existing v0.2.1 and v0.2.2 pins are unchanged.

| Asset | Verified bytes | CI artifact ID |
| --- | ---: | ---: |
| corgi-aarch64-apple-darwin | 4434032 | 12147649916 |
| corgi-x86_64-apple-darwin | 4666512 | 12147804563 |
| corgi-aarch64-unknown-linux-musl | 4396248 | 12148886627 |
| corgi-x86_64-unknown-linux-musl | 5277440 | 12149165499 |

The native x86_64 Linux public binary is static and reports `corgi 0.2.3`.
Controlled socket checks verified intact multiline/Unicode stdin delivery once,
no prompt in held-process argv/environment, safe success/rejection/error and
disconnect handling, no echo or automatic retry, and malformed-input rejection.
Isolated inbox/report checks also passed private creation, legacy/orphan repair,
delivery, compaction and content preservation. CI runs arm64 macOS and both
Linux assets; x86_64 macOS is cross-built without a runtime smoke test.

The published [Omarchy commit `51a7de1012ce01786294fdd6e270ee93703c60da`](https://github.com/zyrre/omarchy-corgi/commit/51a7de1012ce01786294fdd6e270ee93703c60da)
matches the tag's `omarchy/` plus `LICENSE` by paths, modes, blobs and bytes,
retains the previous public history, and passes both manifest validators.
This does not activate an installed widget.

These pins require a separate user review and merge before public prebuilt
installation verification and local activation. Until reviewed pins reach the
public source installation path, v0.2.3 correctly builds from source. The release
tag itself has no v0.2.3 pins.
