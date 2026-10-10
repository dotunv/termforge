# ADR 0013: Updates are discovered through a signed manifest

- Status: Accepted (verification and release tooling); update UI not built
- Date: 2026-10-10

## Context

Beta artifacts are unsigned and users have no way to learn that a new build
exists. An update mechanism is also a remote-code-execution path: whoever can
change what the app downloads controls every user's machine. GitHub release
assets are mutable by anyone with write access to the repository, so transport
integrity (HTTPS) is not enough.

## Decision

- Releases publish `manifest.json` and a detached `manifest.json.sig`. The
  signature is Ed25519 over the exact manifest bytes, hex encoded. The public
  key is compiled into the application; the signing seed exists only as the
  `TF_UPDATE_SIGNING_SEED` CI secret.
- `crates/tf-update` verifies the signature, then parses. It never offers a
  version that is equal to or older than the running one (no downgrade by
  replaying an old manifest), requires `https` asset URLs, and checks asset
  size and SHA-256 against the signed manifest before anything is used.
- The crate does no I/O. Fetching, UI and installation are the caller's job.
- Platform code signing (Authenticode, Developer ID and notarization) is
  applied in the release workflow when its secrets are configured, independent
  of the update signature: OS trust and update trust are separate.
- The first user-facing step is **notify only**: tell the user a verified
  update exists and link to it. There is no silent background install.
- `cargo xtask release-manifest` produces `SHA256SUMS`, `manifest.json` and the
  signature. It refuses to emit a signature without the seed, and the release
  workflow drops an unsigned manifest rather than publishing it.

## Consequences

- Losing the seed means shipping a new public key in a release users install by
  hand. Key rotation is not designed yet; manifests carry a single key.
- A compromised GitHub account can still withhold updates but cannot make the
  app install an attacker's build without the seed.
- Dependencies added: `ed25519-dalek` and `sha2` (both permissively licensed),
  confined to `tf-update` and `xtask`.
