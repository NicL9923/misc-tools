# misc-tools

Custom utilities by Nicolas, licensed under MIT.

## YouTube downloader

A Rust desktop app built with GPUI Kit and yt-dlp. The first version targets Linux.

Paste a single YouTube video link, inspect it, choose video or audio and a destination, then download. The app supports:

- MKV with original video codecs, or H.264/AAC MP4 for compatibility.
- Original audio or high-quality MP3 conversion.
- Resolution caps based on the inspected video's available formats.
- Progress, cancellation of both yt-dlp and FFmpeg, and opening the saved file or folder.
- Remembering the destination folder, with no silent overwrites. Partial files stay available for retry.

MP4-compatible streams may offer fewer resolutions than MKV. A resolution cap is an upper limit, not a promise that every format is available at that resolution.

### Run on Fedora

Install a current stable Rust toolchain using [rustup](https://rustup.rs/), then install the development dependencies:

```bash
sudo dnf install gcc-c++ clang cmake pkgconf-pkg-config fontconfig-devel \
  libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libxcb-devel \
  libX11-devel libXcursor-devel libXi-devel openssl-devel alsa-lib-devel \
  vulkan-loader-devel
```

The app also needs `yt-dlp`, `ffmpeg`, `ffprobe`, and `deno` on PATH. Use a current [yt-dlp installation with its EJS components](https://github.com/yt-dlp/yt-dlp/wiki/EJS), a [Deno runtime](https://docs.deno.com/runtime/getting_started/installation/), and [FFmpeg](https://ffmpeg.org/download.html). Codec availability depends on your FFmpeg build.

```bash
./run-downloader.sh
```

For an optimized binary:

```bash
cargo build --locked --release --package youtube-downloader
./target/release/youtube-downloader
```

The binary uses separately installed download tools. It does not bundle or silently update them. If YouTube extraction stops working, update yt-dlp and its challenge components first. The app reports missing executables at startup.

### Scope and development

Individual public videos, Shorts, and recorded videos are supported. Playlist URLs, channels, live/upcoming streams, and sign-in workflows are outside this version. A video link containing a playlist parameter downloads only that video. Only download media you are permitted to save.

The Cargo workspace keeps this app in `apps/youtube-downloader`. Its download engine owns subprocess arguments, structured output, cancellation, and final-file validation. The GPUI layer handles interaction and presentation. Settings live at `$XDG_CONFIG_HOME/misc-tools/youtube-downloader.json`, falling back to `~/.config`.

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

Engine tests use controlled local processes, including descendants and failing downloads; they do not depend on YouTube availability. See [.agents/tools](.agents/tools/README.md) for manual live checks. Windows and macOS packaging have not been implemented; process cancellation currently uses Unix process groups.

## Planned tools

- **Local image and video generation:** an interface for running open-source models on hardware you own and control. Model selection is pending; separate image and video models are acceptable if needed.

The MIT license covers this repository's original code. Third-party tools and model weights retain their own licenses.

Additional ideas live in [TODOs.md](TODOs.md).
