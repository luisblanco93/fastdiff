#!/usr/bin/env python3
"""Prepare checksums for the paired binary release archives."""

import argparse
import hashlib
from pathlib import Path
import re
import tomllib


def workspace_packages(root: Path) -> set[str]:
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    names = set()
    for member in cargo["workspace"]["members"]:
        for directory in root.glob(member):
            package = tomllib.loads((directory / "Cargo.toml").read_text(encoding="utf-8"))["package"]
            if package["version"] == {"workspace": True}:
                names.add(package["name"])
    if not names:
        raise ValueError("No workspace packages inherit the release version")
    return names


def release_version(root: Path, tag: str | None) -> str:
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    version = cargo["workspace"]["package"]["version"]
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("Release version must be major.minor.patch")
    if tag is not None and tag != f"v{version}":
        raise ValueError(f"Tag {tag} does not match workspace version v{version}")
    packages = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))["package"]
    for name in workspace_packages(root):
        entries = [package for package in packages if package["name"] == name and "source" not in package]
        if len(entries) != 1 or entries[0]["version"] != version:
            raise ValueError(f"Version mismatch in Cargo.lock for {name}")
    return version


def package(root: Path, output: Path, tag: str | None = None) -> Path:
    version = release_version(root, tag)
    assets = [
        output / f"fastdiff-{version}-linux-x64.tar.gz",
        output / f"fastdiff-{version}-macos-arm64.tar.gz",
        output / f"fastdiff-{version}-windows-x64.zip",
    ]
    for asset in assets:
        if not asset.is_file():
            raise ValueError(f"Missing paired app archive: {asset}")
    checksums = []
    for asset in assets:
        with asset.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        checksums.append(f"{digest}  {asset.name}\n")
    checksum_file = output / "SHA256SUMS"
    checksum_file.write_text("".join(checksums), encoding="utf-8")
    return checksum_file


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    parser.add_argument("--tag", help="Version tag to enforce, for example v0.1.1")
    args = parser.parse_args()
    try:
        checksum_file = package(Path(__file__).resolve().parents[1], args.output, args.tag)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Cannot prepare release: {error}\n")
    print(f"Prepared {checksum_file}")


if __name__ == "__main__":
    main()
