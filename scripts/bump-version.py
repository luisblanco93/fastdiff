#!/usr/bin/env python3
"""Update consistent release metadata without rewriting unrelated formatting."""

import argparse
import importlib.util
from pathlib import Path
import re
import tomllib


spec = importlib.util.spec_from_file_location(
    "package_release", Path(__file__).with_name("package-release.py")
)
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


def replace_toml_version(section: str, version: str) -> str:
    result, count = re.subn(
        r'(?m)^([ \t]*version[ \t]*=[ \t]*)"[^"\r\n]+"',
        lambda match: match.group(1) + f'"{version}"', section,
    )
    if count != 1:
        raise ValueError("Expected exactly one version field in release TOML section")
    return result


def prepare_edits(root: Path, version: str) -> dict[Path, bytes]:
    cargo = root / "Cargo.toml"
    sections = re.split(r"(?m)(?=^[ \t]*\[)", cargo.read_bytes().decode("utf-8"))
    header = r"[ \t]*\[workspace\.package\][ \t]*(?:#[^\r\n]*)?(?:\r?\n|$)"
    if sum(bool(re.match(header, section)) for section in sections) != 1:
        raise ValueError("Expected exactly one workspace.package section")
    cargo_data = "".join(
        replace_toml_version(section, version)
        if re.match(header, section) else section
        for section in sections
    ).encode("utf-8")
    names = packager.workspace_packages(root)
    lock = root / "Cargo.lock"
    sections = re.split(r"(?m)(?=^[ \t]*\[\[package\]\])", lock.read_bytes().decode("utf-8"))
    lock_data = "".join(
        update_lock_section(section, names, version) for section in sections
    ).encode("utf-8")
    return {cargo: cargo_data, lock: lock_data}


def update_lock_section(section: str, names: set[str], version: str) -> str:
    if not section.lstrip().startswith("[[package]]"):
        return section
    package = tomllib.loads(section)["package"][0]
    if package["name"] in names and "source" not in package:
        return replace_toml_version(section, version)
    return section


def bump_version(root: Path, version: str) -> bool:
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("Version must be stable major.minor.patch without leading zeroes")
    current = packager.release_version(root, None)
    if tuple(map(int, version.split("."))) < tuple(map(int, current.split("."))):
        raise ValueError(f"Cannot lower version {current} to {version}")
    if version == current:
        return False
    edits = prepare_edits(root, version)
    originals = {path: path.read_bytes() for path in edits}
    attempted = ()
    try:
        for path, data in edits.items():
            attempted = (*attempted, path)
            path.write_bytes(data)
    except OSError as error:
        failures = ()
        for path in reversed(attempted):
            try:
                path.write_bytes(originals[path])
            except OSError as restore_error:
                failures = (*failures, f"{path}: {restore_error}")
        if failures:
            raise OSError(f"{error}; could not restore: {'; '.join(failures)}") from error
        raise
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="Stable version, for example 0.1.3")
    args = parser.parse_args()
    try:
        changed = bump_version(Path(__file__).resolve().parents[1], args.version)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Cannot bump release version: {error}\n")
    print(f"{'Updated' if changed else 'Already at'} version {args.version}")


if __name__ == "__main__":
    main()
