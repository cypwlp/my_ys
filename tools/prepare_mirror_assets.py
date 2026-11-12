#!/usr/bin/env python3
"""Download signed release assets and create a local mirror manifest."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import sys
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

SAFE_NAME = re.compile(r"^[A-Za-z0-9._-]+$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(item: tuple[str, str], output: Path) -> tuple[str, int]:
    name, url = item
    if not SAFE_NAME.fullmatch(name):
        raise ValueError(f"unsafe asset name: {name!r}")
    target = output / name
    partial = output / f".{name}.part"
    request = urllib.request.Request(url, headers={"User-Agent": "KumoArc-Mirror/1.0"})
    with urllib.request.urlopen(request, timeout=300) as response, partial.open("wb") as handle:
        shutil.copyfileobj(response, handle, length=1024 * 1024)
    os.replace(partial, target)
    return name, target.stat().st_size


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <assets.json> <output-dir>", file=sys.stderr)
        return 2

    source = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    version = str(source["version"])
    assets: dict[str, str] = source["assets"]
    output = Path(sys.argv[2])
    output.mkdir(parents=True, exist_ok=True)

    with ThreadPoolExecutor(max_workers=4) as pool:
        jobs = [pool.submit(download, item, output) for item in assets.items()]
        for job in as_completed(jobs):
            name, size = job.result()
            print(f"downloaded {name} ({size} bytes)")

    checksum_file = output / "SHA256SUMS"
    if checksum_file.exists():
        expected: dict[str, str] = {}
        for line in checksum_file.read_text(encoding="utf-8").splitlines():
            match = re.fullmatch(r"([0-9a-fA-F]{64})\s+(.+)", line.strip())
            if match:
                expected[match.group(2)] = match.group(1).lower()
        for name, wanted in expected.items():
            actual = sha256(output / name)
            if actual != wanted:
                raise ValueError(f"checksum mismatch: {name}: {actual} != {wanted}")
            print(f"verified {name}")

    keys = [
        ("windows-setup", lambda n: n.lower().endswith("windows-x64-setup.exe")),
        ("windows-portable", lambda n: n.lower().endswith("windows-x64.exe") and "setup" not in n.lower()),
        ("macos", lambda n: n.lower().endswith(".dmg")),
        ("linux-appimage", lambda n: n.lower().endswith(".appimage")),
        ("linux-deb", lambda n: n.lower().endswith(".deb")),
        ("checksums", lambda n: n.lower() == "sha256sums"),
    ]
    mapping: dict[str, str] = {}
    for key, predicate in keys:
        match = next((name for name in assets if predicate(name)), None)
        if match:
            mapping[key] = match

    manifest = {"version": version, "assets": mapping}
    (output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(manifest, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())