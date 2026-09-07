#!/usr/bin/env python3
"""Install the pinned, local-only Image Studio backend; no root or cloud account."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
import sysconfig

REPO = Path(__file__).resolve().parents[2]
MANIFEST = REPO / 'apps/image-studio/models.json'
ROOT = Path.home() / '.local/opt/misc-tools-comfyui'


def run(*args, **kwargs):
    subprocess.run([str(x) for x in args], check=True, **kwargs)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--list', action='store_true', help='Show pinned downloads and disk requirements without installing')
    parser.add_argument('--skip-models', action='store_true', help='Install runtime only; download models on a later run')
    parser.add_argument('--models', nargs='+', choices=['klein', 'z-image', 'ideogram', 'flux-dev', 'all'], default=['klein', 'z-image'], help='Models to install; the larger non-commercial models are opt-in')
    args = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text())
    selected = list(manifest['models']) if 'all' in args.models else args.models
    names = {name for model in selected for name in manifest['models'][model]}
    files = [item for item in manifest['files'] if item['filename'] in names]
    if any(model in selected for model in ['ideogram', 'flux-dev']):
        print('Ideogram 4 and FLUX.2 Dev weights have non-commercial model licenses. See apps/image-studio/RUNTIME.md. NVFP4 requires a supported NVIDIA Blackwell GPU.')
    for item in files:
        print(f"{item['filename']}: {item['bytes'] / 1e9:.2f} GB", flush=True)
    print(f"Model total: {sum(f['bytes'] for f in files) / 1e9:.2f} GB; allow another 12 GB for Python/CUDA.")
    if args.list:
        return
    ROOT.mkdir(parents=True, exist_ok=True)
    checkout = ROOT / 'ComfyUI'
    if not checkout.exists():
        run('git', 'clone', '--depth', '1', 'https://github.com/Comfy-Org/ComfyUI.git', checkout)
    if subprocess.check_output(['git', '-C', str(checkout), 'status', '--porcelain']).strip():
        raise SystemExit('ComfyUI checkout has local changes; preserve them before running setup.')
    run('git', '-C', checkout, 'fetch', '--depth', '1', 'origin', manifest['comfyui_commit'])
    run('git', '-C', checkout, 'checkout', '--detach', manifest['comfyui_commit'])
    venv = ROOT / 'venv'
    if not (venv / 'bin/python').exists():
        run(sys.executable, '-m', 'venv', venv)
    # Triton builds a small CUDA driver extension against Python.h at first use.
    # Fedora's runtime Python omits these headers; extract its matching devel RPM
    # into our private sysroot without changing installed system packages.
    headers = Path(sysconfig.get_path('include'))
    private_headers = ROOT / 'sysroot/usr/include' / ('python' + sysconfig.get_python_version())
    if not (headers / 'Python.h').exists() and not (private_headers / 'Python.h').exists():
        rpms = ROOT / 'rpms'
        rpms.mkdir(exist_ok=True)
        run('dnf', 'download', '--arch', 'x86_64', '--destdir', rpms, 'python3-devel')
        sysroot = ROOT / 'sysroot'
        sysroot.mkdir(exist_ok=True)
        for rpm in rpms.glob('python3-devel-*.x86_64.rpm'):
            unpack = subprocess.Popen(['rpm2cpio', str(rpm)], stdout=subprocess.PIPE)
            try:
                run('cpio', '-idmu', '--no-absolute-filenames', stdin=unpack.stdout, cwd=sysroot, stdout=subprocess.DEVNULL)
            finally:
                unpack.stdout.close()
            if unpack.wait() != 0:
                raise SystemExit('Could not unpack Python development headers')
        if not (private_headers / 'Python.h').exists():
            raise SystemExit('The downloaded Python headers do not match this Python version')
    pip = venv / 'bin/pip'
    run(pip, 'install', 'torch==2.14.0', 'torchvision==0.29.0', 'torchaudio==2.11.0', '--index-url', 'https://download.pytorch.org/whl/cu130')
    run(pip, 'install', '-r', checkout / 'requirements.txt')
    if not args.skip_models:
        for item in files:
            target = checkout / 'models' / item['directory'] / item['filename']
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.exists():
                if digest(target) != item['sha256']:
                    raise SystemExit(f'Existing model checksum differs: {target}. Move it aside before retrying.')
                continue
            # Hugging Face's Xet transport downloads large files in parallel chunks.
            # Stage outside ComfyUI's model folders until our pinned hash verifies.
            staging = ROOT / 'downloads' / item['repo'].replace('/', '--') / item['revision']
            partial = staging / item['path']
            print(f"Downloading {item['filename']}…", flush=True)
            run(venv / 'bin/hf', 'download', item['repo'], item['path'], '--revision', item['revision'], '--local-dir', staging)
            if digest(partial) != item['sha256']:
                raise SystemExit(f'Checksum failed: {partial}. Remove that partial download and retry.')
            partial.rename(target)
            # Discard an incomplete download left by the previous curl installer.
            target.with_name(target.name + '.part').unlink(missing_ok=True)
    (ROOT / 'models.json').write_text(MANIFEST.read_text())
    launcher = ROOT / 'run.sh'
    launcher.write_text('#!/bin/sh\nset -eu\nexport C_INCLUDE_PATH=' + shlex.quote(str(private_headers)) + '\ncd ' + shlex.quote(str(checkout)) + '\nexec ' + shlex.quote(str(venv / 'bin/python')) + ' main.py --listen 127.0.0.1 --port 8190 --disable-api-nodes --disable-all-custom-nodes --reserve-vram 2.5 --cache-none --fast-disk\n')
    launcher.chmod(0o755)
    units = Path.home() / '.config/systemd/user'
    units.mkdir(parents=True, exist_ok=True)
    # User service is started on demand, never enabled at login.
    unit = units / 'misc-tools-comfyui.service'
    unit.write_text('[Unit]\nDescription=Image Studio local ComfyUI backend\n\n[Service]\nType=simple\nExecStart="' + str(launcher).replace('%', '%%').replace('"', '\\"') + '"\nRestart=no\nMemoryHigh=14G\nMemoryMax=20G\nMemorySwapMax=2G\nEnvironment=HF_HUB_OFFLINE=1\nEnvironment=TRANSFORMERS_OFFLINE=1\n\n')
    run('systemctl', '--user', 'daemon-reload')
    print('Installed. Start with: systemctl --user start misc-tools-comfyui.service')
    print('Local interface: http://127.0.0.1:8190')


if __name__ == '__main__':
    main()
