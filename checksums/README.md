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
