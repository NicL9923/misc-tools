#!/usr/bin/env python3
"""Install the release build and its desktop launchers for the current Linux user."""
from pathlib import Path
import argparse
import os
import shlex
import shutil
import subprocess

repo = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--app', choices=['youtube-downloader', 'image-studio', 'file-converter'], default='youtube-downloader')
app = parser.parse_args().app
studio = app == 'image-studio'
converter = app == 'file-converter'
name = 'File Converter' if converter else 'Image Studio' if studio else 'YouTube Downloader'
icon = 'icon.svg' if studio or converter else 'icon.png'
root = Path.home() / '.local/opt' / app
binary = repo / 'target/release' / app
if not binary.is_file():
    raise SystemExit(f'Build first: .agents/tools/cargo.sh build --locked --release -p {app}')


def copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + '.new')
    shutil.copy2(source, temporary)
    temporary.replace(destination)


copy(binary, root / app)
copy(repo / 'apps' / app / 'assets' / icon, root / icon)
copy(repo / 'LICENSE', root / 'LICENSE')
# Reuse project-local runtime downloads when present; otherwise use installed tools.
for tool in (() if studio or converter else ('yt-dlp', 'deno')):
    source = repo / '.tools/bin' / tool
    if source.is_file():
        copy(source, root / 'bin' / tool)
launcher = Path.home() / '.local/bin' / app
launcher.parent.mkdir(parents=True, exist_ok=True)
startup = 'systemctl --user start misc-tools-comfyui.service || true\n' if studio else ''
launcher.write_text(
    '#!/usr/bin/env bash\nset -euo pipefail\n'
    f'export PATH={shlex.quote(str(root / "bin"))}:"$PATH"\n'
    + startup + f'exec {shlex.quote(str(root / app))} "$@"\n'
)
launcher.chmod(0o755)


def desktop_quote(value):
    # Desktop Exec quoting differs from shell quoting.
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%') + '"'


data_home = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share'))
entry = data_home / 'applications' / f'misc-tools-{app}.desktop'
entry.parent.mkdir(parents=True, exist_ok=True)
entry.write_text(
    f'[Desktop Entry]\nType=Application\nName={name}\n'
    f'Comment={"Convert images, media and documents locally" if converter else "Generate images locally with multiple models" if studio else "Download YouTube video or audio"}\n'
    f'Exec={desktop_quote(launcher)}\nIcon={root / icon}\n'
    f'Terminal=false\nCategories={"Utility;" if converter else "Graphics;" if studio else "AudioVideo;"}\n'
    f'Keywords={"convert;image;audio;video;PDF;document;" if converter else "images;AI;generation;models;" if studio else "YouTube;video;audio;download;"}\n'
    f'StartupWMClass=misc-tools-{app}\n'
)
if shutil.which('xdg-user-dir'):
    desktop = Path(subprocess.check_output(['xdg-user-dir', 'DESKTOP'], text=True).strip())
    if desktop.is_dir() and desktop != Path.home():
        copy(entry, desktop / entry.name)
        (desktop / entry.name).chmod(0o755)
for command in (['desktop-file-validate', str(entry)], ['update-desktop-database', str(entry.parent)]):
    if shutil.which(command[0]):
        subprocess.run(command, check=True)
print(f'Installed: {launcher}\nRestart the app to use the new build.')
