# Developer tools

- `cargo.sh`: run Cargo from the workspace root, using optional project-local tools in `.tools/env.sh`.
- `unpack-build-deps.py`: unpack downloaded Fedora build RPMs into this checkout's ignored `.tools/sysroot`, without changing system packages.

For a live engine check without a GUI, use `./.agents/tools/cargo.sh run --example probe -- URL [DIRECTORY] [mkv|mp4|audio|mp3]`. With only a URL it inspects metadata; with a directory it downloads. Use a video you have permission to download. Live checks are intentionally outside CI.
