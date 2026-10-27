#!/usr/bin/env python3
"""Verify the paired release assets and create or resume a draft GitHub Release."""

import argparse
from collections.abc import Callable, Sequence
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile
import zipfile

Gh = Callable[[Sequence[str]], str]

HELPER_NAME = "fastdiff"
VALIDATION_NOTE = (
    "Automated checks: the CI run above (Linux, macOS, and Windows tests, release builds, "
    "native window smoke checks, packaging). Not covered by CI: launching this paired "
    "build from Windows into WSL; manual smoke results are recorded in docs/validation.md."
)


def sha256(stream) -> str:
    return hashlib.file_digest(stream, "sha256").hexdigest()


def file_sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return sha256(stream)


def check_member_names(names: list[str], root: str) -> None:
    """Reject unsafe, escaping, or (case-insensitively) duplicate archive member names."""
    seen = set()
    for name in names:
        parts = name.rstrip("/").split("/")
        if "\\" in name or name.startswith("/") or ".." in parts or "" in parts or parts[0] != root:
            raise ValueError(f"Unsafe archive member name: {name!r}")
        key = name.rstrip("/").casefold()
        if key in seen:
            raise ValueError(f"Duplicate archive member: {name!r}")
        seen.add(key)


LICENSE_NAME = "licenses/material-icons-LICENSE"
THEMES_NOTICE_NAME = "licenses/color-themes-NOTICE"


def verify_tar_archive(path: Path, root: str, label: str) -> str:
    """Check a Unix archive's names and contents; return the SHA-256 of its helper."""
    with tarfile.open(path) as archive:
        members = archive.getmembers()
        check_member_names([m.name for m in members], root)
        for member in members:
            if not (member.isfile() or member.isdir()):
                raise ValueError(f"{label} archive member {member.name!r} is not a file or directory")
        files = {m.name for m in members if m.isfile()}
        expected = {f"{root}/bin/fastdiff", f"{root}/bin/fastdiff-gui", f"{root}/{LICENSE_NAME}",
                    f"{root}/{THEMES_NOTICE_NAME}"}
        if files != expected:
            raise ValueError(f"Unexpected {label} archive contents: {sorted(files ^ expected)}")
        return sha256(archive.extractfile(f"{root}/bin/{HELPER_NAME}"))


def verify_assets(directory: Path, version: str) -> dict[str, Path]:
    """Check the four release assets and the paired WSL helper; return name -> path."""
    linux = f"fastdiff-{version}-linux-x64.tar.gz"
    macos = f"fastdiff-{version}-macos-arm64.tar.gz"
    windows = f"fastdiff-{version}-windows-x64.zip"
    sums = "SHA256SUMS"
    present = {path.name for path in directory.iterdir()} if directory.is_dir() else set()
    if present != {linux, macos, windows, sums}:
        raise ValueError(
            f"Release assets must be exactly {sorted((linux, macos, windows, sums))}; found {sorted(present)}"
        )
    assets = {name: directory / name for name in (linux, macos, windows, sums)}
    recorded = {}
    for line in assets[sums].read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line)
        if not match or match[2] in recorded:
            raise ValueError(f"Malformed SHA256SUMS line: {line!r}")
        recorded[match[2]] = match[1]
    if set(recorded) != {linux, macos, windows}:
        raise ValueError("SHA256SUMS must list exactly the Linux, macOS, and Windows archives")
    for name, digest in recorded.items():
        if file_sha256(assets[name]) != digest:
            raise ValueError(f"Checksum mismatch for {name}")
    linux_root, windows_root = linux.removesuffix(".tar.gz"), windows.removesuffix(".zip")
    linux_helper = verify_tar_archive(assets[linux], linux_root, "Linux")
    # The macOS helper is a Mach-O binary, so it is checked for layout only, not paired.
    verify_tar_archive(assets[macos], macos.removesuffix(".tar.gz"), "macOS")
    with zipfile.ZipFile(assets[windows]) as archive:
        entries = archive.infolist()
        check_member_names([e.filename for e in entries], windows_root)
        for entry in entries:
            if (entry.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError(f"Windows archive entry {entry.filename!r} is a symbolic link")
        files = {e.filename for e in entries if not e.filename.endswith("/")}
        expected = {f"{windows_root}/bin/fastdiff.exe", f"{windows_root}/bin/fastdiff-gui.exe",
                    f"{windows_root}/wsl/fastdiff", f"{windows_root}/{LICENSE_NAME}",
                    f"{windows_root}/{THEMES_NOTICE_NAME}"}
        if files != expected:
            raise ValueError(f"Unexpected Windows archive contents: {sorted(files ^ expected)}")
        with archive.open(f"{windows_root}/wsl/{HELPER_NAME}") as stream:
            windows_helper = sha256(stream)
    if linux_helper != windows_helper:
        raise ValueError("The Windows archive's WSL helper differs from the Linux archive's helper")
    return assets


def decode_stream(text: str) -> list:
    """Decode concatenated JSON values, as printed by `gh api --paginate`."""
    decoder, values, index = json.JSONDecoder(), [], 0
    while index < len(text):
        if text[index].isspace():
            index += 1
            continue
        value, index = decoder.raw_decode(text, index)
        values.extend(value if isinstance(value, list) else [value])
    return values


def release_body(notes: str, commit: str, run_url: str) -> str:
    return (
        f"{notes.strip()}\n\n---\n\n"
        f"Source commit: `{commit}`\n\nCI run: {run_url}\n\n{VALIDATION_NOTE}\n"
    )


def asset_matches(remote: dict, path: Path) -> bool:
    if remote.get("size") != path.stat().st_size:
        return False
    digest = remote.get("digest")
    return digest is None or digest == f"sha256:{file_sha256(path)}"


def sync_draft(gh: Gh, repo: str, tag: str, commit: str, body: str, assets: dict[str, Path]) -> str:
    """Create the draft for `tag`, or resume a matching one; never touch a published release."""
    listing = decode_stream(gh(["api", "--paginate", f"repos/{repo}/releases?per_page=100"]))
    matches = [release for release in listing if release["tag_name"] == tag]
    if len(matches) > 1:
        raise ValueError(f"More than one release uses tag {tag}; resolve this manually")
    if matches and not matches[0]["draft"]:
        raise ValueError(f"Release {tag} is already published and its assets are immutable")
    if matches:
        release = json.loads(gh([
            "api", "-X", "PATCH", f"repos/{repo}/releases/{matches[0]['id']}",
            "-f", f"body={body}", "-F", "prerelease=false",
        ]))
    else:
        release = json.loads(gh([
            "api", "-X", "POST", f"repos/{repo}/releases",
            "-f", f"tag_name={tag}", "-f", f"target_commitish={commit}", "-f", f"name={tag}",
            "-f", f"body={body}", "-F", "draft=true", "-F", "prerelease=false",
        ]))
    existing = {asset["name"]: asset for asset in release.get("assets", [])}
    unexpected = set(existing) - set(assets)
    if unexpected:
        raise ValueError(f"Draft {tag} has unexpected assets: {sorted(unexpected)}")
    pending = []
    for name, path in assets.items():
        remote = existing.get(name)
        if remote is None:
            pending.append(path)
        elif remote.get("state") != "uploaded":
            gh(["api", "-X", "DELETE", f"repos/{repo}/releases/assets/{remote['id']}"])
            pending.append(path)
        elif not asset_matches(remote, path):
            raise ValueError(f"Draft asset {name} conflicts with the verified local asset")
    if pending:
        gh(["release", "upload", tag, "--repo", repo, *map(str, pending)])
    final = json.loads(gh(["api", f"repos/{repo}/releases/{release['id']}"]))
    if not final["draft"] or final["prerelease"]:
        raise ValueError(f"Release {tag} is not a non-prerelease draft")
    remote = {asset["name"]: asset for asset in final.get("assets", [])}
    if set(remote) != set(assets) or not all(
        remote[name].get("state") == "uploaded" and asset_matches(remote[name], path)
        for name, path in assets.items()
    ):
        raise ValueError(f"Draft {tag} does not contain exactly the verified assets")
    return final["html_url"]


def run_gh(arguments: Sequence[str]) -> str:
    try:
        result = subprocess.run(["gh", *arguments], capture_output=True, text=True)
    except OSError as error:
        raise ValueError(f"Cannot run gh: {error}") from error
    if result.returncode != 0:
        raise ValueError(f"gh {' '.join(arguments[:3])} failed: {result.stderr.strip()}")
    return result.stdout


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True, help="owner/name")
    parser.add_argument("--commit", required=True, help="Source commit SHA of the tag")
    parser.add_argument("--run-url", required=True)
    parser.add_argument("--assets", type=Path, default=Path("dist"))
    parser.add_argument("--notes-file", type=Path, required=True)
    args = parser.parse_args()
    try:
        if not re.fullmatch(r"v\d+\.\d+\.\d+", args.tag):
            raise ValueError(f"Tag {args.tag} must be a stable vMAJOR.MINOR.PATCH version")
        assets = verify_assets(args.assets, args.tag[1:])
        notes = args.notes_file.read_text(encoding="utf-8")
        body = release_body(notes, args.commit, args.run_url)
        url = sync_draft(run_gh, args.repo, args.tag, args.commit, body, assets)
    except (OSError, ValueError, KeyError, json.JSONDecodeError, tarfile.TarError,
            zipfile.BadZipFile) as error:
        parser.exit(1, f"Cannot prepare draft release: {error}\n")
    print(f"Draft release ready: {url}")


if __name__ == "__main__":
    main()
