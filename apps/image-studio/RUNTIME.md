# Local generation runtime

Image Studio uses a separate ComfyUI process on `http://127.0.0.1:8190`.
It runs installed models on the local NVIDIA GPU; API/cloud nodes and custom nodes
are disabled. The Rust app submits one image at a time and owns the persistent
queue and gallery. ComfyUI is GPL-3.0; its separately installed source is not
relicensed by this MIT project. The default Klein and Z-Image weights are
Apache-2.0. Optional Ideogram 4 and FLUX.2 Dev weights have non-commercial model
licenses. Their upstream repositories are linked below and pinned in `models.json`.

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

### Optional larger models

```sh
python3 .agents/tools/setup-image-runtime.py --models ideogram flux-dev --list
python3 .agents/tools/setup-image-runtime.py --models ideogram flux-dev
```

This adds about 50.60 GB beyond the default models and shares the existing FLUX.2
VAE. `--models all` installs all four recipes; without `--models`, setup still
installs only Klein and Z-Image. These additional recipes use NVFP4 weights and
require a supported NVIDIA Blackwell GPU and NVFP4 kernels. They are not portable
presets for older NVIDIA cards, AMD, or Apple GPUs.

FLUX.2 Dev retains the full 32B generator in a 21.04 GB NVFP4 file, with a separate
12.28 GB mixed FP4 Mistral encoder. It exceeds the 16 GB GPU's capacity and relies
on NVMe-backed offloading. Ideogram uses two 5.49 GB NVFP4 generators and a 6.31 GB
Qwen3-VL encoder. Start with one 1024px image per model. Large batches still run
sequentially, but these recipes do substantially more work per image than Klein
or Z-Image.

The app's MIT license does not replace either model's license. Review the
[Ideogram model license](https://huggingface.co/ideogram-ai/ideogram-4-fp8)
and [FLUX.2 Dev license and usage policy](https://huggingface.co/black-forest-labs/FLUX.2-dev-NVFP4).
These weights are downloaded separately, never bundled in the app or repository.

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

`workflows/flux-dev.json` uses BFL's full NVFP4 Dev weights with 50 Euler steps,
Flux2Scheduler, distilled guidance 4, and CFG 1. It does not apply a Turbo LoRA.
`workflows/ideogram.json` implements `V4_QUALITY_48`: 45 Euler steps with dual-model
guidance 7, followed by three steps with guidance 3, using the same latent and
noise schedule. Its schedule uses mu 0 and std 1.5. Unconditional inference is
image-only. All nodes are built into the pinned ComfyUI runtime.

Ideogram was trained on structured JSON captions. Image Studio wraps ordinary
text in a minimal caption without inventing scene details; a JSON object with a
`high_level_description` string passes through unchanged. Detailed structured
captions can improve layout control. There is no hosted Magic Prompt call or
additional prompt-expansion model. The original prompt and exact submitted
workflow are both retained in each image's metadata.

The backend commit and each model repository revision, size, and SHA-256 are in
`models.json`. Python dependencies follow that ComfyUI revision, with PyTorch
2.14.0 CUDA 13.0 pinned; the entire transitive Python dependency set is not locked.

Sources:
- [Official Klein workflow](https://docs.comfy.org/tutorials/flux/flux-2-klein)
- [Official Z-Image-Turbo workflow](https://docs.comfy.org/tutorials/image/z-image/z-image-turbo)
- [ComfyUI source](https://github.com/Comfy-Org/ComfyUI)
- [Klein 4B FP8 weights](https://huggingface.co/black-forest-labs/FLUX.2-klein-4b-fp8)
- [Z-Image-Turbo components](https://huggingface.co/Comfy-Org/z_image_turbo)
- [BFL full FLUX.2 Dev NVFP4 weights](https://huggingface.co/black-forest-labs/FLUX.2-dev-NVFP4)
- [ComfyUI FLUX.2 Dev encoders](https://huggingface.co/Comfy-Org/flux2-dev)
- [Ideogram Quality sampler parameters](https://github.com/ideogram-oss/ideogram4/blob/main/docs/inference.md)
- [ComfyUI Ideogram weights](https://huggingface.co/Comfy-Org/Ideogram-4)
- [Official Ideogram workflow](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/image_ideogram4_t2i.json)

## Verified locally

On NicolasDESKTOP (Fedora 44, RTX 5070 Ti 16 GB, 32 GB RAM), both API recipes
produced real 1024×1024 PNGs. Klein took 4.18 seconds and Z-Image-Turbo took
10.18 seconds including loading
and API polling. The initial RAM-backed configuration thrashed during Z-Image;
`--fast-disk` resolved that, so it is part of the installed launcher. These are
single-run observations, not a controlled benchmark.
