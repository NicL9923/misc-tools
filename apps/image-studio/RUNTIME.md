# Local generation runtime

Image Studio uses a separate ComfyUI process on `http://127.0.0.1:8190`.
It runs installed models on the local NVIDIA GPU; API/cloud nodes and custom nodes
are disabled. The Rust app submits one image at a time and owns the persistent
queue and gallery. ComfyUI is GPL-3.0; its separately installed source is not
relicensed by this MIT project. Model licenses are Apache-2.0; see their upstream
repositories linked in `models.json`.

## Install on Fedora with an NVIDIA GPU

Requires Fedora x86_64, Python 3.14, Git, curl, dnf download, rpm2cpio, cpio, GCC,
working NVIDIA drivers, and roughly 40 GB free disk.
Python/CUDA packages live in a dedicated virtual environment; no sudo is needed.
Missing Python development headers are extracted from Fedora’s matching RPM into
a private sysroot for Triton’s CUDA driver compilation.

```sh
# Review the exact model downloads first (25.10 GB total).
python3 .agents/tools/setup-image-runtime.py --list
python3 .agents/tools/setup-image-runtime.py
systemctl --user start misc-tools-comfyui.service
```

Use `--skip-models` to install only the runtime. Rerun without that flag to install
the pinned model files. Files download sequentially to resumable `.part` files and
are SHA-256 verified before being made available to ComfyUI. Existing models with
unexpected hashes are preserved and reported rather than overwritten.

The install root is `~/.local/opt/misc-tools-comfyui`. The user service starts on
demand, not at login. It reserves 2.5 GB VRAM for the desktop, uses dynamic VRAM with `--fast-disk`
NVMe-backed offloading, and disables node-result caching to limit host memory growth.
The service has a 20 GB host-memory limit and a 2 GB swap limit; it stops on
failure instead of repeatedly reloading a model that does not fit.
Stop it to release all GPU/host memory:

```sh
systemctl --user stop misc-tools-comfyui.service
journalctl --user -u misc-tools-comfyui.service -n 60
```

Model downloads are needed only during setup. The service sets Hugging Face and
Transformers offline flags; prompts and generated files stay on this computer.
Do not expose its unauthenticated API to the network.

## Pinned workflows

`workflows/klein.json` and `workflows/z-image.json` are API-format text-to-image
recipes, flattened from the official ComfyUI templates at commit
`db9d5859d09c21a2d4101a1c18f64fc2f70e4fa4`. Klein uses the distilled
4B FP8 model, four Euler steps, CFG 1, and Flux2Scheduler. Z-Image-Turbo uses eight
res_multistep steps, the simple scheduler, CFG 1, and AuraFlow shift 3. Both share
the Qwen3 4B text encoder, but use their own CLIP mode and VAE.

The backend commit and each model repository revision, size, and SHA-256 are in
`models.json`. Python dependencies follow that ComfyUI revision, with PyTorch
2.14.0 CUDA 13.0 pinned; the entire transitive Python dependency set is not locked.

Sources:
- [Official Klein workflow](https://docs.comfy.org/tutorials/flux/flux-2-klein)
- [Official Z-Image-Turbo workflow](https://docs.comfy.org/tutorials/image/z-image/z-image-turbo)
- [ComfyUI source](https://github.com/Comfy-Org/ComfyUI)
- [Klein 4B FP8 weights](https://huggingface.co/black-forest-labs/FLUX.2-klein-4b-fp8)
- [Z-Image-Turbo components](https://huggingface.co/Comfy-Org/z_image_turbo)

## Verified locally

On NicolasDESKTOP (Fedora 44, RTX 5070 Ti 16 GB, 32 GB RAM), both API recipes
produced real 1024×1024 PNGs. Klein took 4.18 seconds and Z-Image-Turbo took
10.18 seconds including loading
and API polling. The initial RAM-backed configuration thrashed during Z-Image;
`--fast-disk` resolved that, so it is part of the installed launcher. These are
single-run observations, not a controlled benchmark.
