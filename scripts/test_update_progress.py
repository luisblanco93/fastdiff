"""Offline checks for the progress and status output of `dff --update` (LIT-40)."""

import contextlib
import io
import unittest
from unittest.mock import patch

import test_plugin_runtime as runtime_tests
from test_plugin_runtime import VERSION, fake_download, runtime

MIB = 1024 * 1024


class Terminal(io.StringIO):
    def __init__(self, interactive: bool) -> None:
        super().__init__()
        self.interactive = interactive

    def isatty(self) -> bool:
        return self.interactive


class FakeResponse:
    def __init__(self, body: bytes, length=None, fail_after: int | None = None) -> None:
        self.headers = {} if length is None else {"Content-Length": length}
        self.stream = io.BytesIO(body)
        self.fail_after = fail_after
        self.served = 0

    def __enter__(self):
        return self

    def __exit__(self, *exception) -> None:
        return None

    def read(self, size: int) -> bytes:
        if self.fail_after is not None and self.served >= self.fail_after:
            raise OSError("connection reset")
        chunk = self.stream.read(size)
        self.served += len(chunk)
        return chunk


class ProgressFormattingTests(unittest.TestCase):
    def test_known_total_shows_bar_percent_and_sizes(self):
        self.assertEqual(runtime.format_progress("a.tar.gz", MIB * 3 // 2, 3 * MIB),
                         "a.tar.gz [########--------]  50%  1.5 MiB / 3.0 MiB")

    def test_bar_runs_from_empty_to_full(self):
        empty = runtime.format_progress("a", 0, MIB)
        full = runtime.format_progress("a", MIB, MIB)
        self.assertIn("[" + "-" * runtime.BAR_WIDTH + "]   0%", empty)
        self.assertIn("[" + "#" * runtime.BAR_WIDTH + "] 100%", full)

    def test_percent_never_exceeds_one_hundred(self):
        line = runtime.format_progress("a", 5 * MIB, MIB)
        self.assertIn("[" + "#" * runtime.BAR_WIDTH + "] 100%", line)

    def test_progress_line_is_ascii_and_fits_a_terminal(self):
        line = runtime.format_progress("fastdiff-1.3.1-windows-x64.zip", 100 * MIB, 100 * MIB)
        self.assertTrue(line.isascii())
        self.assertLess(len(line), 80)

    def test_unknown_total_shows_only_received_bytes(self):
        for total in (None, 0):
            with self.subTest(total=total):
                self.assertEqual(runtime.format_progress("a", MIB, total), "a  1.0 MiB")

    def test_content_length_accepts_only_positive_integers(self):
        for value, expected in [("1024", 1024), ("0", None), ("-5", None), ("abc", None), (None, None)]:
            with self.subTest(value=value):
                headers = {} if value is None else {"Content-Length": value}
                self.assertEqual(runtime.content_length(headers), expected)


class DownloadProgressTests(unittest.TestCase):
    def test_terminal_output_is_one_updating_line_ended_with_a_newline(self):
        stream, now = Terminal(True), iter([0.0, 0.05, 0.2])
        progress = runtime.DownloadProgress("a", 4 * MIB, stream, clock=lambda: next(now))
        progress.start()
        for received in (MIB, 2 * MIB, 3 * MIB):
            progress.update(received)
        progress.finish(4 * MIB)
        output = stream.getvalue()
        self.assertEqual(output.count("\r"), 3)  # the redraw inside the interval was skipped
        self.assertEqual(output.count("\n"), 1)
        self.assertTrue(output.endswith("4.0 MiB / 4.0 MiB\n"))
        self.assertIn("100%", output.rsplit("\r", 1)[-1])

    def test_redirected_output_has_discrete_lines_without_carriage_returns(self):
        stream = Terminal(False)
        progress = runtime.DownloadProgress("a", 4 * MIB, stream)
        progress.start()
        progress.update(MIB)
        progress.finish(4 * MIB)
        self.assertEqual(stream.getvalue(), "Downloading a...\nDownloaded a (4.0 MiB)\n")

    def test_abort_ends_an_open_line_only_once(self):
        stream = Terminal(True)
        progress = runtime.DownloadProgress("a", None, stream)
        progress.abort()
        self.assertEqual(stream.getvalue(), "")
        progress.update(MIB)
        progress.abort()
        progress.abort()
        self.assertEqual(stream.getvalue().count("\n"), 1)

    def test_stream_without_isatty_is_treated_as_redirected(self):
        self.assertFalse(runtime.DownloadProgress("a", None, object()).interactive)

    def test_a_broken_stream_turns_progress_off_without_raising(self):
        class Broken(Terminal):
            writes = 0

            def write(self, text):
                self.writes += 1
                raise BrokenPipeError("closed")

        stream = Broken(True)
        progress = runtime.DownloadProgress("a", MIB, stream, clock=lambda: 0.0)
        progress.start()
        progress.update(MIB)
        progress.finish(MIB)
        progress.abort()
        self.assertEqual(stream.writes, 1)  # disabled after the first failure

    def test_missing_stderr_disables_progress_and_steps(self):
        with patch.object(runtime.sys, "stderr", None):
            progress = runtime.DownloadProgress("a", MIB)
            progress.start()
            progress.finish(MIB)
            runtime.report_step("Verifying checksums")
        self.assertFalse(progress.enabled)

    def test_report_step_ignores_a_closed_stream(self):
        stream = Terminal(False)
        stream.close()
        with patch.object(runtime.sys, "stderr", stream):
            runtime.report_step("Verifying checksums")


class DownloadFileTests(unittest.TestCase):
    setUp = runtime_tests.PluginRuntimeTests.setUp

    def download(self, response: FakeResponse, interactive: bool, name: str | None = "a.tar.gz"):
        stream = Terminal(interactive)
        destination = self.parent / "download.bin"
        with patch.object(runtime, "urlopen", return_value=response), contextlib.redirect_stderr(stream):
            runtime.download_file("https://example.invalid/a", destination, name=name)
        return destination, stream.getvalue()

    def test_known_length_writes_every_byte_and_ends_at_one_hundred_percent(self):
        body = b"x" * (2 * MIB + 5)
        destination, output = self.download(FakeResponse(body, str(len(body))), True)
        self.assertEqual(destination.read_bytes(), body)
        self.assertIn("100%", output.rsplit("\r", 1)[-1])

    def test_missing_content_length_still_downloads_and_reports_bytes(self):
        body = b"x" * MIB
        destination, output = self.download(FakeResponse(body), True)
        self.assertEqual(destination.read_bytes(), body)
        self.assertIn("a.tar.gz  1.0 MiB", output)
        self.assertNotIn("%", output)

    def test_missing_content_length_prints_a_plain_line_when_redirected(self):
        destination, output = self.download(FakeResponse(b"x" * MIB), False)
        self.assertEqual(output, "Downloading a.tar.gz...\nDownloaded a.tar.gz (1.0 MiB)\n")
        self.assertEqual(destination.stat().st_size, MIB)

    def test_download_without_a_name_is_silent(self):
        body = b"checksums"
        destination, output = self.download(FakeResponse(body, str(len(body))), True, name=None)
        self.assertEqual(destination.read_bytes(), body)
        self.assertEqual(output, "")

    def test_interrupted_download_ends_the_line_and_propagates_the_error(self):
        response = FakeResponse(b"x" * (3 * runtime.DOWNLOAD_CHUNK), fail_after=runtime.DOWNLOAD_CHUNK)
        stream = Terminal(True)
        with patch.object(runtime, "urlopen", return_value=response), contextlib.redirect_stderr(stream):
            with self.assertRaisesRegex(OSError, "connection reset"):
                runtime.download_file("https://example.invalid/a", self.parent / "partial.bin", name="a.tar.gz")
        self.assertTrue(stream.getvalue().endswith("\n"))

    def test_download_completes_when_stderr_is_a_closed_pipe(self):
        body = b"x" * MIB
        stream = Terminal(True)
        stream.close()
        destination = self.parent / "download.bin"
        with patch.object(runtime, "urlopen", return_value=FakeResponse(body, str(len(body)))):
            with contextlib.redirect_stderr(stream):
                runtime.download_file("https://example.invalid/a", destination, name="a.tar.gz")
        self.assertEqual(destination.read_bytes(), body)

    def test_asset_download_names_archives_but_not_the_checksum_file(self):
        name = runtime.archive_name(VERSION, "linux")
        with patch.object(runtime, "download_file") as download:
            runtime.download_assets(VERSION, [name], self.parent / "assets")
        names = {call.args[0].rsplit("/", 1)[1]: call.kwargs.get("name") for call in download.call_args_list}
        self.assertEqual(names, {"SHA256SUMS": None, name: name})


class SetupStatusTests(unittest.TestCase):
    setUp = runtime_tests.PluginRuntimeTests.setUp

    def setup_output(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                runtime.setup(VERSION, "linux", self.root)
        return stdout.getvalue(), stderr.getvalue()

    def test_fresh_install_reports_each_step_on_stderr_and_keeps_the_final_line_on_stdout(self):
        stdout, stderr = self.setup_output()
        self.assertEqual(stderr.splitlines(), [
            f"Installing fastdiff {VERSION}",
            "Verifying checksums",
            f"Installing {runtime.archive_name(VERSION, 'linux')}",
            "Refreshing the dff command",
        ])
        self.assertEqual(stdout.splitlines()[-1], f"Installed fastdiff {VERSION}; invoke dff to open Git changes")

    def test_already_installed_path_prints_no_progress(self):
        self.setup_output()
        stdout, stderr = self.setup_output()
        self.assertEqual(stderr, "")
        self.assertEqual(stdout.splitlines()[-1], f"fastdiff {VERSION} is already installed")


if __name__ == "__main__":
    unittest.main()
