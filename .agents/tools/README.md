# Developer tools

- `capture-x11.py`: capture one app window on an isolated X11 display for visual verification.
- `setup-image-runtime.py`: install the pinned local ComfyUI runtime and verified Klein/Z-Image weights; `--list` shows downloads, `--skip-models` installs only the runtime.
- `install-desktop.py`: install a release binary, icon, and menu/desktop launchers; defaults to the downloader, or use `--app image-studio`. Copies project-local yt-dlp and Deno for the downloader when present.
- `cargo.sh`: run Cargo from the workspace root, using optional project-local tools in `.tools/env.sh`.
- `unpack-build-deps.py`: unpack downloaded Fedora build RPMs into this checkout's ignored `.tools/sysroot`, without changing system packages.

For a live engine check without a GUI, use `./.agents/tools/cargo.sh run --example probe -- URL [DIRECTORY] [mkv|mp4|audio|mp3]`. With only a URL it inspects metadata; with a directory it downloads. Use a video you have permission to download. Live checks are intentionally outside CI.

- `setup-image-runtime.py`: install the pinned local ComfyUI service and checksum-verified image models; `--list` previews downloads.
- `probe-image-runtime.py`: submit one real image per model to the idle local backend and report elapsed time and saved outputs.

For an Image Studio engine smoke test, use `./.agents/tools/cargo.sh run --example generate -- DIRECTORY klein|z-image|both COUNT PROMPT`. This queues real images through the same engine as the GUI. Use a separate DIRECTORY for test state and results.
