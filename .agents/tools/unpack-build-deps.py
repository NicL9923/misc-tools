#!/usr/bin/env python3
"""Relocate Fedora RPMs downloaded into .tools/rpms for this checkout's build."""
from pathlib import Path
import subprocess

repo = Path(__file__).resolve().parents[2]
root = repo / ".tools/sysroot"
root.mkdir(parents=True, exist_ok=True)
for rpm in sorted((repo / ".tools/rpms").glob("*.rpm")):
    if not rpm.name.endswith((".x86_64.rpm", ".noarch.rpm")):
        continue
    unpack = subprocess.Popen(["rpm2cpio", str(rpm)], stdout=subprocess.PIPE)
    subprocess.run(["cpio", "-idmu", "--quiet"], cwd=root, stdin=unpack.stdout, check=True)
    if unpack.wait() != 0:
        raise SystemExit(f"Could not extract {rpm.name}")
for link in (root / "usr/lib64").glob("*.so"):
    if link.is_symlink() and not link.exists():
        host = Path("/usr/lib64") / link.readlink().name
        if host.exists():
            link.unlink()
            link.symlink_to(host)
for pc in (root / "usr/lib64/pkgconfig").glob("*.pc"):
    pc.write_text(pc.read_text().replace("prefix=/usr", "prefix=" + str(root / "usr")))
