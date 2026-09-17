# misc-tools

Custom utilities by Nicolas, licensed under MIT.

## File converter

A Rust/GPUI app for batch conversions on Linux. Add multiple files, choose an
output format for each, and save beside the originals or in another folder.
Conversions run locally. Existing files keep their names; new results receive
numbered suffixes when needed. You can cancel a batch and retry failed items.

| Files | Outputs | Engine |
| --- | --- | --- |
| Images, including HEIC, AVIF, SVG, JPEG, PNG, WebP, GIF, TIFF | PNG, JPEG, WebP, AVIF, TIFF, BMP, GIF, ICO, PDF | ImageMagick |
| Audio | MP3, M4A, FLAC, WAV, OGG, Opus | FFmpeg |
| Video | MP4, MKV, WebM, MOV, or extracted audio | FFmpeg |
| Word, OpenDocument text, RTF | PDF, DOCX, ODT, RTF, TXT | LibreOffice |
| Excel, OpenDocument sheets, CSV | XLSX, ODS, PDF, CSV | LibreOffice |
| PowerPoint, OpenDocument slides | PDF, PPTX, ODP | LibreOffice |
| Markdown, HTML, plain text, EPUB | HTML, DOCX, ODT, Markdown, TXT, EPUB | Pandoc |
| PDF | Text, first-page PNG or JPEG | Poppler |

Image conversions use the first frame or page. JPEG gets a white background
where the source is transparent. SVG becomes a raster image. CSV exports one
sheet without formatting. PDF text extraction does not perform OCR.
Media conversion uses the first video and audio tracks; it omits subtitles and
extra tracks. Document layout can change, so check the result.

Codec and image-delegate availability depends on the installed engines.
The app reports missing executables and conversion failures per file.
It does not edit PDFs, vectorize images, convert archives, or remove DRM.

### Run the converter

Use the Linux build dependencies listed under the downloader below. Install
the conversion engines for the file types you need:

```bash
sudo dnf install ImageMagick libreoffice-writer libreoffice-calc \
  libreoffice-impress poppler-utils pandoc
# FFmpeg also needs the encoders used by your chosen formats, including
# libx264 for MP4/MOV/MKV and libmp3lame for MP3.
./run-file-converter.sh
# File paths can also populate the queue at launch:
./run-file-converter.sh ~/Pictures/photo.heic ~/Documents/report.docx
```

For a desktop launcher:

```bash
./.agents/tools/cargo.sh build --locked --release --package file-converter
./.agents/tools/install-desktop.py --app file-converter
```

The engine stages files inside the destination folder and publishes completed
outputs atomically without overwriting existing files. Allow room for a copy
of the input plus the output. LibreOffice uses an isolated profile for each
conversion. Closing the app cancels the active conversion. Completed files stay
saved; the queue itself is not persisted.

Engine usage follows the [FFmpeg command-line documentation](https://ffmpeg.org/ffmpeg.html)
and [LibreOffice conversion parameters](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html).

### Converter checks

```bash
./.agents/tools/cargo.sh test --locked -p file-converter
# Explicit live checks require the engines and codecs listed by each test:
./.agents/tools/cargo.sh test --locked -p file-converter --test conversions -- --ignored
# Convert one file without the GUI:
./.agents/tools/cargo.sh run --locked -p file-converter --example convert -- SOURCE TARGET_EXTENSION [FOLDER]
```

## Image Studio

A Rust/GPUI app for generating images locally through ComfyUI. Choose FLUX.2
Klein 4B, Z-Image-Turbo, Ideogram 4.0 Quality, or the full FLUX.2 Dev 32B model,
and generate several variations from one prompt.
Jobs run one at a time in model groups. A gallery groups results by model and
supports favorites, fresh-seed regeneration, full-size viewing, and exporting copies.

The app saves its queue under `~/.local/state/misc-tools/image-studio`, honoring
`XDG_STATE_HOME`, and images under `~/Pictures/Image Studio`. Each image has a
JSON sidecar with its prompt, seed, dimensions, workflow, and pinned model-file
identities. Retry preserves the seed; Regenerate chooses a new one.

Pending jobs survive restarts. A running job reconnects using its saved ComfyUI
prompt ID. If the backend loses a job, it becomes a retryable failure instead of
silently generating a duplicate. Pause stops new jobs; cancelling a running image
lets that image finish and discards its result. This avoids interrupting someone
else's ComfyUI job. Completed images remain available.

### Install on Fedora with an NVIDIA GPU

Set up the local runtime, then build and install the app. The runtime download is
about 25 GB of weights plus Python/CUDA packages. It lives outside the repository.

```bash
./.agents/tools/setup-image-runtime.py --list
./.agents/tools/setup-image-runtime.py
./.agents/tools/cargo.sh build --locked --release --package image-studio
./.agents/tools/install-desktop.py --app image-studio
```

Launch **Image Studio** from KDE's application menu or desktop, or run
`~/.local/bin/image-studio`. For development, use `./run-image-studio.sh`.
The launcher starts the local backend on demand; the service is not enabled at login.
Use **Engine & models** in the app to inspect availability or restart the service.
The service listens only on `127.0.0.1:8190`, with cloud API nodes and custom nodes
turned off. Generation runs offline after installation. See
[the runtime notes](apps/image-studio/RUNTIME.md) for dependencies and troubleshooting.

Ideogram and FLUX.2 Dev are optional larger downloads using NVFP4 quantization
for supported NVIDIA Blackwell GPUs, including the RTX 5070 Ti:

```bash
./.agents/tools/setup-image-runtime.py --models ideogram flux-dev --list
./.agents/tools/setup-image-runtime.py --models ideogram flux-dev
```

These add about 51 GB of weights to the default installation. FLUX.2 Dev retains
the full 32B model and uses disk offloading on a 16 GB GPU. Ideogram uses its
48-step Quality preset. Both use non-commercial model licenses; check the
[runtime notes](apps/image-studio/RUNTIME.md) before commercial use.

The first version supports text-to-image with these four recipes. Image editing,
video generation, and installing arbitrary models through the GUI are outside
this version. ComfyUI remains a separate GPL-licensed dependency; our app code is
MIT. The default Klein and Z-Image model releases are Apache 2.0; the optional
Ideogram and FLUX.2 Dev weights retain their own licenses.

## YouTube downloader

A Rust desktop app built with GPUI Kit and yt-dlp. The first version targets Linux.

Paste a single YouTube video link, inspect it, choose video or audio and a destination, then download. The app supports:

- MKV with original video codecs, or H.264/AAC MP4 for compatibility.
- Original audio or high-quality MP3 conversion.
- Resolution caps based on the inspected video's available formats.
- Progress, cancellation of both yt-dlp and FFmpeg, and opening the saved file or folder.
- Remembering the destination folder, with no overwrites. An existing filename produces an explicit conflict instead of returning an older download.

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

For a user-local installation with an application-menu entry and desktop shortcut:

```bash
./.agents/tools/install-desktop.py
```

The installer copies the release binary and icon into `~/.local/opt/youtube-downloader`
and creates `~/.local/bin/youtube-downloader`. Restart the app after reinstalling.
If this checkout has project-local yt-dlp and Deno binaries, it copies them into the
app's private `bin` directory; otherwise they must already be on PATH.

The binary uses separately installed download tools. It does not bundle or silently update them. If YouTube extraction stops working, update yt-dlp and its challenge components first. The app reports missing executables at startup.

Downloads and conversions run in `.misc-tools-partials` inside the selected folder. Completed files move atomically into the destination without replacing existing files. Cancelled or failed conversions are discarded; download fragments remain for retry. You may remove `.misc-tools-partials` when no downloads are running if you no longer need those fragments.

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

- **Local image editing and video generation:** extend Image Studio with reference images and a separately selected video model.

The MIT license covers this repository's original code. Third-party tools and model weights retain their own licenses.

Additional ideas live in [TODOs.md](TODOs.md).
