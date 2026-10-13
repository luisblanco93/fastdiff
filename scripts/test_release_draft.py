"""Offline checks for release asset verification and draft creation."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release_draft", ROOT / "scripts/release-draft.py")
draft = importlib.util.module_from_spec(spec)
spec.loader.exec_module(draft)

VERSION = "1.0.0"
LINUX = f"fastdiff-{VERSION}-linux-x64"
WINDOWS = f"fastdiff-{VERSION}-windows-x64"
REPO = "owner/fastdiff"
TAG = f"v{VERSION}"


def write_assets(directory: Path, helper=b"helper", windows_helper=None, extra_linux=()):
    """Create a valid asset set; options corrupt one aspect of it."""
    with tarfile.open(directory / f"{LINUX}.tar.gz", "w:gz") as archive:
        members = {f"{LINUX}/bin/fastdiff": helper, f"{LINUX}/bin/fastdiff-gui": b"gui",
                   f"{LINUX}/licenses/material-icons-LICENSE": b"license"}
        for name in extra_linux:
            members[name] = b"unexpected"
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    with zipfile.ZipFile(directory / f"{WINDOWS}.zip", "w") as archive:
        archive.writestr(f"{WINDOWS}/bin/fastdiff.exe", b"launcher")
        archive.writestr(f"{WINDOWS}/bin/fastdiff-gui.exe", b"gui")
        archive.writestr(f"{WINDOWS}/wsl/fastdiff", helper if windows_helper is None else windows_helper)
        archive.writestr(f"{WINDOWS}/licenses/material-icons-LICENSE", b"license")
    write_sums(directory)


def write_sums(directory: Path):
    lines = []
    for name in (f"{LINUX}.tar.gz", f"{WINDOWS}.zip"):
        lines.append(f"{hashlib.sha256((directory / name).read_bytes()).hexdigest()}  {name}\n")
    (directory / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")


class VerifyAssetsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)

    def test_valid_paired_assets_are_accepted(self):
        write_assets(self.directory)
        self.assertEqual(set(draft.verify_assets(self.directory, VERSION)),
                         {f"{LINUX}.tar.gz", f"{WINDOWS}.zip", "SHA256SUMS"})

    def test_missing_or_extra_files_are_rejected(self):
        write_assets(self.directory)
        (self.directory / "notes.txt").write_text("extra")
        with self.assertRaisesRegex(ValueError, "exactly"):
            draft.verify_assets(self.directory, VERSION)
        (self.directory / "notes.txt").unlink()
        (self.directory / f"{WINDOWS}.zip").unlink()
        with self.assertRaisesRegex(ValueError, "exactly"):
            draft.verify_assets(self.directory, VERSION)

    def test_corrupted_archive_fails_checksum(self):
        write_assets(self.directory)
        path = self.directory / f"{LINUX}.tar.gz"
        path.write_bytes(path.read_bytes() + b"\0")
        with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
            draft.verify_assets(self.directory, VERSION)

    def test_malformed_or_incomplete_checksums_are_rejected(self):
        write_assets(self.directory)
        sums = self.directory / "SHA256SUMS"
        first = sums.read_text().splitlines()[0]
        sums.write_text(first + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "exactly the Linux and Windows"):
            draft.verify_assets(self.directory, VERSION)
        sums.write_text("not a checksum\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Malformed"):
            draft.verify_assets(self.directory, VERSION)

    def test_unexpected_archive_member_is_rejected(self):
        write_assets(self.directory, extra_linux=[f"{LINUX}/bin/extra"])
        with self.assertRaisesRegex(ValueError, "Unexpected Linux archive"):
            draft.verify_assets(self.directory, VERSION)

    def rewrite_linux(self, add):
        """Append one crafted member to an otherwise valid Linux archive and refresh checksums."""
        write_assets(self.directory)
        path = self.directory / f"{LINUX}.tar.gz"
        with tarfile.open(path) as source:
            members = [(m, source.extractfile(m).read() if m.isfile() else None) for m in source]
        with tarfile.open(path, "w:gz") as archive:
            for member, data in members:
                archive.addfile(member, io.BytesIO(data) if data is not None else None)
            add(archive)
        write_sums(self.directory)

    def test_non_regular_duplicate_and_escaping_members_are_rejected(self):
        def symlink(archive):
            info = tarfile.TarInfo(f"{LINUX}/bin/link")
            info.type, info.linkname = tarfile.SYMTYPE, "fastdiff"
            archive.addfile(info)

        def traversal(archive):
            info = tarfile.TarInfo(f"{LINUX}/../escape")
            info.type = tarfile.DIRTYPE
            archive.addfile(info)

        def duplicate(archive):
            info = tarfile.TarInfo(f"{LINUX}/bin/FASTDIFF-GUI")
            info.size = 3
            archive.addfile(info, io.BytesIO(b"gui"))

        for add in (symlink, traversal, duplicate):
            with self.subTest(case=add.__name__):
                self.rewrite_linux(add)
                with self.assertRaises(ValueError):
                    draft.verify_assets(self.directory, VERSION)

    def test_duplicate_or_symlink_zip_entries_are_rejected(self):
        write_assets(self.directory)
        path = self.directory / f"{WINDOWS}.zip"
        with zipfile.ZipFile(path, "a") as archive:
            with self.assertWarns(UserWarning):
                archive.writestr(f"{WINDOWS}/bin/fastdiff-gui.exe", b"other bytes")
        write_sums(self.directory)
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            draft.verify_assets(self.directory, VERSION)
        write_assets(self.directory)
        with zipfile.ZipFile(path, "a") as archive:
            info = zipfile.ZipInfo(f"{WINDOWS}/bin/link")
            info.external_attr = 0o120777 << 16
            archive.writestr(info, "fastdiff")
        write_sums(self.directory)
        with self.assertRaisesRegex(ValueError, "symbolic link"):
            draft.verify_assets(self.directory, VERSION)

    def test_unpaired_wsl_helper_is_rejected(self):
        write_assets(self.directory, windows_helper=b"another build")
        with self.assertRaisesRegex(ValueError, "helper differs"):
            draft.verify_assets(self.directory, VERSION)


class FakeGh:
    """Minimal stateful model of the GitHub release endpoints used by the script."""

    def __init__(self, releases=()):
        self.releases = {r["id"]: r for r in releases}
        self.calls = []
        self.drop_uploads = False

    @staticmethod
    def fields(args):
        pairs = args[args.index("-f"):] if "-f" in args else []
        return {pairs[i + 1].split("=", 1)[0]: pairs[i + 1].split("=", 1)[1]
                for i in range(len(pairs) - 1) if pairs[i] in ("-f", "-F")}

    def __call__(self, args):
        self.calls.append(list(args))
        if args[:2] == ["api", "--paginate"]:
            return json.dumps(list(self.releases.values()))
        if args[:3] == ["api", "-X", "POST"]:
            fields = self.fields(args)
            release = {"id": 100 + len(self.releases), "tag_name": fields["tag_name"],
                       "draft": fields["draft"] == "true", "prerelease": fields["prerelease"] == "true",
                       "body": fields["body"], "assets": [], "html_url": "https://example.invalid/draft"}
            self.releases[release["id"]] = release
            return json.dumps(release)
        if args[:3] == ["api", "-X", "PATCH"]:
            release = self.releases[int(args[3].rsplit("/", 1)[1])]
            release["body"] = self.fields(args)["body"]
            return json.dumps(release)
        if args[:3] == ["api", "-X", "DELETE"]:
            asset_id = int(args[3].rsplit("/", 1)[1])
            for release in self.releases.values():
                release["assets"] = [a for a in release["assets"] if a["id"] != asset_id]
            return ""
        if args[:2] == ["release", "upload"]:
            release = next(r for r in self.releases.values() if r["tag_name"] == args[2])
            if not self.drop_uploads:
                for path in map(Path, args[5:]):
                    release["assets"].append({
                        "id": len(release["assets"]) + 900, "name": path.name, "state": "uploaded",
                        "size": path.stat().st_size,
                        "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()})
            return ""
        if args[0] == "api" and len(args) == 2:
            return json.dumps(self.releases[int(args[1].rsplit("/", 1)[1])])
        raise AssertionError(f"Unexpected gh call: {args}")

    def uploads(self):
        return [call for call in self.calls if call[:2] == ["release", "upload"]]


class SyncDraftTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        write_assets(self.directory)
        self.assets = draft.verify_assets(self.directory, VERSION)

    def sync(self, gh):
        return draft.sync_draft(gh, REPO, TAG, "a" * 40, "notes", self.assets)

    def release(self, **overrides):
        base = {"id": 7, "tag_name": TAG, "draft": True, "prerelease": False,
                "body": "old", "assets": [], "html_url": "https://example.invalid/draft"}
        return base | overrides

    def remote_asset(self, name, **overrides):
        path = self.assets[name]
        base = {"id": 50, "name": name, "state": "uploaded", "size": path.stat().st_size,
                "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()}
        return base | overrides

    def test_creates_a_non_prerelease_draft_with_all_three_assets(self):
        gh = FakeGh()
        self.assertEqual(self.sync(gh), "https://example.invalid/draft")
        created = next(c for c in gh.calls if c[:3] == ["api", "-X", "POST"])
        self.assertIn("draft=true", created)
        self.assertIn("prerelease=false", created)
        self.assertIn(f"target_commitish={'a' * 40}", created)
        self.assertEqual(len(gh.uploads()), 1)
        self.assertEqual(len(gh.uploads()[0][5:]), 3)

    def test_retry_resumes_a_matching_incomplete_draft(self):
        name = f"{LINUX}.tar.gz"
        gh = FakeGh([self.release(assets=[self.remote_asset(name)])])
        self.sync(gh)
        uploaded = {Path(path).name for path in gh.uploads()[0][5:]}
        self.assertEqual(uploaded, {f"{WINDOWS}.zip", "SHA256SUMS"})
        self.assertFalse([c for c in gh.calls if c[:3] == ["api", "-X", "POST"]])

    def test_resume_patch_does_not_resend_draft_state(self):
        gh = FakeGh([self.release(assets=[self.remote_asset(f"{LINUX}.tar.gz")])])
        self.sync(gh)
        patch = next(c for c in gh.calls if c[:3] == ["api", "-X", "PATCH"])
        self.assertNotIn("draft=true", patch)

    def test_complete_matching_draft_uploads_nothing(self):
        gh = FakeGh([self.release(assets=[
            self.remote_asset(name) | {"id": index} for index, name in enumerate(self.assets)])])
        self.sync(gh)
        self.assertEqual(gh.uploads(), [])

    def test_incomplete_remote_asset_is_replaced(self):
        name = f"{LINUX}.tar.gz"
        gh = FakeGh([self.release(assets=[self.remote_asset(name, state="starter", id=3)])])
        self.sync(gh)
        self.assertTrue([c for c in gh.calls if c[:3] == ["api", "-X", "DELETE"]])
        self.assertIn(name, {Path(path).name for path in gh.uploads()[0][5:]})

    def test_published_release_is_never_modified(self):
        gh = FakeGh([self.release(draft=False)])
        with self.assertRaisesRegex(ValueError, "already published"):
            self.sync(gh)
        self.assertEqual([c for c in gh.calls if c[0] != "api" or c[1] == "-X"], [])

    def test_conflicting_or_unexpected_draft_assets_are_rejected(self):
        name = f"{LINUX}.tar.gz"
        for asset, message in (
            (self.remote_asset(name, size=1), "conflicts"),
            (self.remote_asset(name, digest="sha256:" + "0" * 64), "conflicts"),
            ({"id": 9, "name": "stray.bin", "state": "uploaded", "size": 1}, "unexpected"),
        ):
            with self.subTest(message=message):
                gh = FakeGh([self.release(assets=[asset])])
                with self.assertRaisesRegex(ValueError, message):
                    self.sync(gh)
                self.assertEqual(gh.uploads(), [])

    def test_ambiguous_tag_is_rejected(self):
        gh = FakeGh([self.release(), self.release(id=8)])
        with self.assertRaisesRegex(ValueError, "More than one"):
            self.sync(gh)

    def test_upload_that_does_not_land_fails_final_verification(self):
        gh = FakeGh()
        gh.drop_uploads = True
        with self.assertRaisesRegex(ValueError, "exactly the verified assets"):
            self.sync(gh)

    def test_release_body_records_commit_run_and_limits(self):
        body = draft.release_body("- Notes", "b" * 40, "https://example.invalid/run/1")
        for expected in ("- Notes", "b" * 40, "https://example.invalid/run/1", "Not covered by CI"):
            self.assertIn(expected, body)
        self.assertNotIn("draft", body.lower())


class DecodeStreamTests(unittest.TestCase):
    def test_concatenated_pages_and_objects_are_flattened(self):
        text = '[{"a": 1}, {"a": 2}]\n[{"a": 3}]\n{"a": 4}\n'
        self.assertEqual([item["a"] for item in draft.decode_stream(text)], [1, 2, 3, 4])


if __name__ == "__main__":
    unittest.main()
