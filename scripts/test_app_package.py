"""Check the Linux application archive with executable and license fixtures."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(sys.platform == "linux", "Linux packaging requires Linux")
class AppPackageTests(unittest.TestCase):
    def test_linux_archive_contains_binaries_and_material_license(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
        package = f"fastdiff-{version}-linux-x64"
        binaries = {"fastdiff": b"helper fixture\n", "fastdiff-gui": b"GUI fixture\n"}
        license_content = b"Material icons license fixture\n"
        expected = {
            f"{package}/bin/fastdiff",
            f"{package}/bin/fastdiff-gui",
            f"{package}/licenses/material-icons-LICENSE",
        }
        with tempfile.TemporaryDirectory(prefix="fastdiff app package ") as directory:
            base = Path(directory)
            script = base / "scripts/package-linux.sh"
            script.parent.mkdir()
            script.write_bytes((ROOT / "scripts/package-linux.sh").read_bytes())
            license_path = base / "crates/fastdiff-gui/assets/material/LICENSE"
            license_path.parent.mkdir(parents=True)
            license_path.write_bytes(license_content)
            binary_dir = base / "binaries"
            binary_dir.mkdir()
            tools_dir = base / "tools"
            tools_dir.mkdir()
            cargo = tools_dir / "cargo"
            metadata = json.dumps({"packages": [{"name": "fastdiff-cli", "version": version}]})
            cargo.write_text(f"#!/usr/bin/env python3\nprint({metadata!r})\n", encoding="utf-8")
            cargo.chmod(0o755)
            for name, content in binaries.items():
                path = binary_dir / name
                path.write_bytes(content)
                path.chmod(0o755)
            result = subprocess.run(
                ["bash", str(script), str(binary_dir), str(base / "output")],
                cwd=ROOT, capture_output=True, text=True,
                env=os.environ | {"PATH": f"{tools_dir}{os.pathsep}{os.environ['PATH']}"},
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            with tarfile.open(base / "output" / f"{package}.tar.gz") as archive:
                self.assertEqual({member.name for member in archive if member.isfile()}, expected)
                member = archive.getmember(f"{package}/licenses/material-icons-LICENSE")
                self.assertEqual(archive.extractfile(member).read(), license_content)
                self.assertEqual(member.mode & 0o777, 0o644)
                for name, content in binaries.items():
                    member = archive.getmember(f"{package}/bin/{name}")
                    self.assertEqual(archive.extractfile(member).read(), content)
                    self.assertEqual(member.mode & 0o777, 0o755)


if __name__ == "__main__":
    unittest.main()
