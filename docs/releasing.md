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

## Current beta limitations

- Artifacts are not code-signed or notarized. Windows SmartScreen and macOS
  Gatekeeper may warn; signing identities and protected CI secrets are required
  before calling these production installers.
- The Linux artifact is a native tarball, not an AppImage or distro package,
  and relies on the system graphics/font libraries listed in the README.
- There is no auto-update channel yet.
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
