# Beta release process

TermForge beta artifacts are built on their native operating systems. A Linux
build is not evidence that the Windows PTY bundle or macOS application layout
works, so `.github/workflows/release.yml` builds Windows, macOS and Linux in
parallel.

## Local bundle

Run:

```sh
cargo xtask ci
cargo xtask dist
```

`dist` performs a locked release build of `termforge`, `forged` and `tf`, then
stages `target/dist/termforge-<version>-<os>-<arch>`. The executables remain
siblings because the UI discovers and starts `forged` beside itself. Windows
bundles also contain the pinned `conpty.dll` and `OpenConsole.exe`; missing
files fail the build. The Windows executable embeds the TermForge icon. macOS
uses a minimal `TermForge.app` layout with a generated icon set. Linux bundles
include a freedesktop launcher and hicolor icon under `share/`.

Before publishing, run the staged `tf version` and `forged doctor`. The desktop
application still requires an interactive display and must be smoke-tested on
each native OS.

## Publish

Push a version tag such as `v0.1.0-beta.1`. The release workflow:

1. builds and archives all three native bundles;
2. runs daemon diagnostics on Windows and Linux;
3. uploads workflow artifacts; and
4. creates a GitHub prerelease with generated notes when every platform passes.

Manual workflow runs build artifacts without publishing a GitHub release.

## Signing secrets

All are optional; a missing group skips that platform's signing step.

| Secret | Used for |
|---|---|
| `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD` | Authenticode signing of `termforge.exe`, `forged.exe`, `tf.exe` |
| `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY` | Developer ID signing |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | notarization and stapling |
| `TF_UPDATE_SIGNING_SEED` | 64 hex chars (`openssl rand -hex 32`); signs `manifest.json` |

Run `TF_UPDATE_SIGNING_SEED=<seed> cargo xtask update-pubkey` to get the public
key that the application must embed. Keep the seed out of the repository.

## Current beta limitations

- Artifacts are signed and notarized only when the signing secrets below are
  configured; otherwise Windows SmartScreen and macOS Gatekeeper will warn. The
  signing steps in `release.yml` have not been exercised against real
  certificates yet: do a dry run with a tag on a fork before relying on them.
- The Linux artifact is a native tarball, not an AppImage or distro package,
  and relies on the system graphics/font libraries listed in the README.
- There is no in-app updater. Releases publish a signed `manifest.json` (see
  [ADR 0013](adr/0013-signed-updates.md)), and `tf-update` can verify it, but
  nothing in the application fetches or shows it yet, and the public key has
  not been embedded.
- A green hosted build validates compilation and bundle structure; the release
  owner must still complete the native smoke checklist below.

## Native smoke checklist

- Launch TermForge and type into the first shell.
- Close and reopen the UI; confirm the hosted session and scrollback return.
- Restart `forged`; confirm the project shell, CWD and dimensions are recreated.
- Run `tf status blocked --kind question --message "Continue?"` and verify the
  attention state.
- Create, list and complete a durable task with `tf task`.
- Exercise copy, paste, search, links, resize and a full-screen application.
- On Windows, confirm `forged doctor` reports the bundled ConPTY.
