#!/usr/bin/env python3
"""Install the release build and its desktop launchers for the current Linux user."""
from pathlib import Path
import os
import shlex
import shutil
import subprocess

repo = Path(__file__).resolve().parents[2]
root = Path.home() / '.local/opt/youtube-downloader'
binary = repo / 'target/release/youtube-downloader'
if not binary.is_file():
    raise SystemExit('Build first: .agents/tools/cargo.sh build --locked --release -p youtube-downloader')


def copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + '.new')
    shutil.copy2(source, temporary)
    temporary.replace(destination)


copy(binary, root / 'youtube-downloader')
copy(repo / 'apps/youtube-downloader/assets/icon.png', root / 'icon.png')
copy(repo / 'LICENSE', root / 'LICENSE')
# Reuse project-local runtime downloads when present; otherwise use installed tools.
for name in ('yt-dlp', 'deno'):
    source = repo / '.tools/bin' / name
    if source.is_file():
        copy(source, root / 'bin' / name)
launcher = Path.home() / '.local/bin/youtube-downloader'
launcher.parent.mkdir(parents=True, exist_ok=True)
launcher.write_text(
    '#!/usr/bin/env bash\nset -euo pipefail\n'
    f'export PATH={shlex.quote(str(root / "bin"))}:"$PATH"\n'
    f'exec {shlex.quote(str(root / "youtube-downloader"))} "$@"\n'
)
launcher.chmod(0o755)


def desktop_quote(value):
    # Desktop Exec quoting differs from shell quoting.
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%') + '"'


data_home = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share'))
entry = data_home / 'applications/misc-tools-youtube-downloader.desktop'
entry.parent.mkdir(parents=True, exist_ok=True)
entry.write_text(
    '[Desktop Entry]\nType=Application\nName=YouTube Downloader\n'
    'Comment=Download YouTube video or audio\n'
    f'Exec={desktop_quote(launcher)}\nIcon={root / "icon.png"}\n'
    'Terminal=false\nCategories=AudioVideo;\nKeywords=YouTube;video;audio;download;\n'
    'StartupWMClass=misc-tools-youtube-downloader\n'
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
