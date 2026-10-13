"""Resource aggregation checks over fake process tables; no GUI, WSL, or Windows."""

import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

from benchmark_resources import (
    ComponentReading, LinuxProcess, ProcessSample, Reading, ResourceProbe,
    ResourceUnavailable, TreeReading, WSL_TABLE_COMMAND, aggregate, helper_tree_source,
    is_helper, linux_tree_source, local_table_reader, parse_stat, parse_wsl_table,
    probe_for, read_proc_table, subtree, summarize_resources, unavailable_tree,
    windows_tree_source, wsl_table_reader,
)
from benchmark_windows import WindowsSampler, parse_tree_response

TICKS = 100
PAGE = 4096


def stat_line(pid, ppid, utime=0, stime=0, rss_pages=0, comm="app", state="S",
              cutime=0, cstime=0, start=0):
    # Fields after the command: state, ppid, ..., utime(11), stime(12), cutime(13),
    # cstime(14), ..., starttime(19), ..., rss(21).
    fields = [state, str(ppid)] + ["0"] * 9 + [str(utime), str(stime), str(cutime), str(cstime)] \
        + ["0"] * 4 + [str(start), "0", str(rss_pages)] + ["0"] * 30
    return f"{pid} ({comm}) " + " ".join(fields) + "\n"


def sample(pid, parent, rss, cpu, reaped=None, start=None):
    return ProcessSample(pid, parent, rss, cpu, reaped, start)


def component(at, *processes, unreadable=()):
    return ComponentReading(at, TreeReading(tuple(processes), tuple(unreadable)))


def missing(reason):
    return ComponentReading(None, None, reason)


class ProcParsingTests(unittest.TestCase):
    def test_stat_with_spaces_and_parentheses_in_the_command_name(self) -> None:
        parsed = parse_stat(stat_line(77, 5, 250, 50, 300, comm="my (odd) app"), TICKS, PAGE)
        self.assertEqual(parsed, ProcessSample(77, 5, 300 * PAGE, 3.0, 0.0, 0.0))

    def test_stat_reports_reaped_children_cpu_and_start_time(self) -> None:
        parsed = parse_stat(stat_line(77, 5, 100, 20, 1, cutime=150, cstime=30, start=4200),
                            TICKS, PAGE)
        self.assertEqual(parsed.cpu_seconds, 1.2)  # utime + stime only
        self.assertEqual(parsed.reaped_cpu_seconds, 1.8)  # cutime + cstime
        self.assertEqual(parsed.start_seconds, 42.0)

    def test_zombie_and_malformed_stat(self) -> None:
        self.assertIsNone(parse_stat(stat_line(9, 1, state="Z"), TICKS, PAGE))
        with self.assertRaises(ValueError):
            parse_stat("12 (short) S 1 2", TICKS, PAGE)

    @unittest.skipIf(os.name == "nt", "Linux /proc layout")
    def test_fake_proc_tree_selects_descendants_and_skips_vanished_entries(self) -> None:
        ticks, page = os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            table = {10: (1, 2), 11: (10, 1), 12: (10, 0), 13: (11, 3), 20: (1, 9)}
            for pid, (ppid, seconds) in table.items():
                (root / str(pid)).mkdir()
                (root / str(pid) / "stat").write_text(
                    stat_line(pid, ppid, utime=seconds * ticks, rss_pages=10 + pid))
            (root / "30").mkdir()  # Exited between listing and reading its stat.
            (root / "self").mkdir()
            reading = linux_tree_source(root)(10)
            self.assertEqual([s.pid for s in reading.processes], [10, 11, 12, 13])
            self.assertEqual(reading.processes[0].rss_bytes, 20 * page)
            self.assertEqual(reading.processes[0].cpu_seconds, 2.0)
            with self.assertRaises(ResourceUnavailable):
                linux_tree_source(root)(999)
            full = read_proc_table(root, ticks, page)
            self.assertEqual(sorted(entry.sample.pid for entry in full), [10, 11, 12, 13, 20])

    def test_subtree_follows_parent_links_only(self) -> None:
        table = [LinuxProcess(sample(pid, ppid, 0, 0.0)) for pid, ppid in
                 [(1, 0), (10, 1), (11, 10), (12, 11), (20, 1)]]
        self.assertEqual([s.pid for s in subtree(table, [11])], [11, 12])
        self.assertEqual(subtree(table, [999]), ())


class AggregationTests(unittest.TestCase):
    def readings(self):
        before = Reading(
            component(10.0, sample(100, 1, 0, 1.0), sample(101, 100, 0, 0.2),
                      sample(102, 100, 0, 0.5)),
            component(10.0, sample(500, 1, 0, 0.0), sample(501, 500, 0, 0.1)))
        after = Reading(
            component(12.0, sample(100, 1, 100, 1.5), sample(101, 100, 40, 0.4),
                      sample(103, 100, 10, 0.1)),
            component(12.0, sample(500, 1, 300, 0.3), sample(501, 500, 20, 0.1)))
        return before, after

    def test_gui_only_and_total_are_reported_separately(self) -> None:
        before, after = self.readings()
        result = aggregate(before, after, 100, gui_rss_bytes=1000)
        gui_only, total = result["gui_only"], result["total"]
        self.assertEqual(gui_only["rss_bytes"], 1000)
        self.assertAlmostEqual(gui_only["idle_cpu_percent"], 25.0)
        self.assertEqual(gui_only["process_count"], 1)
        self.assertEqual(gui_only["unavailable"], [])
        # root 0.5 s + child 101 0.2 s + new child 103 0.1 s + helper 0.3 s over 2 s;
        # exited 102 has no reaped-children counter here, so its CPU is not counted.
        self.assertAlmostEqual(total["idle_cpu_percent"], 55.0)
        self.assertEqual(total["rss_bytes"], 1000 + 40 + 10 + 300 + 20)
        self.assertEqual(total["process_count"], 5)
        self.assertEqual(total["components"], ["gui", "gui_descendants", "wsl_helper"])
        self.assertEqual(total["unavailable"], [])
        self.assertIn("103", total["notes"][0])  # Started inside the window.

    def test_a_process_that_exited_inside_the_window_makes_the_run_incomplete(self) -> None:
        before, after = self.readings()
        total = aggregate(before, after, 100, gui_rss_bytes=1000)["total"]
        self.assertFalse(total["complete"])
        self.assertEqual(set(total["incomplete_reasons"]), {"gui_descendants"})
        reason = total["incomplete_reasons"]["gui_descendants"]
        self.assertIn("102", reason)
        self.assertIn("not counted", reason)  # No reaped-children counter: loss is explicit.
        self.assertEqual(total["unavailable"], [])
        self.assertIsNotNone(total["idle_cpu_percent"])  # Measured, but flagged.

    def test_linux_reaped_children_cpu_counts_exited_and_short_lived_children(self) -> None:
        # Child 101 had used 0.2 s before the window and 0.5 s in total when it exited
        # (0.3 s inside); an unseen child used 0.1 s. The root's reaped counter holds both
        # lifetimes (0.6 s), of which the 0.2 s already visible in the first reading is removed.
        before = Reading(component(0.0, sample(100, 1, 0, 1.0, 0.0), sample(101, 100, 0, 0.2, 0.0)), None)
        after = Reading(component(2.0, sample(100, 1, 0, 1.5, 0.6)), None)
        result = aggregate(before, after, 100, gui_rss_bytes=10)
        self.assertAlmostEqual(result["gui_only"]["idle_cpu_percent"], 25.0)  # Root's own CPU.
        total = result["total"]
        self.assertAlmostEqual(total["idle_cpu_percent"], 25.0 + 20.0)  # + 0.4 s of children.
        self.assertFalse(total["complete"])
        reason = total["incomplete_reasons"]["gui_descendants"]
        self.assertIn("[101]", reason)
        self.assertIn("0.400 CPU s", reason)
        self.assertNotIn("not counted", reason)

    def test_short_lived_children_invisible_to_both_readings_are_still_flagged(self) -> None:
        before = Reading(component(0.0, sample(100, 1, 0, 1.0, 0.0)), None)
        after = Reading(component(2.0, sample(100, 1, 0, 1.0, 0.1)), None)
        total = aggregate(before, after, 100, gui_rss_bytes=10)["total"]
        self.assertAlmostEqual(total["idle_cpu_percent"], 5.0)
        self.assertFalse(total["complete"])
        self.assertIn("0.100 CPU s", total["incomplete_reasons"]["gui_descendants"])
        self.assertNotIn("exited during", total["incomplete_reasons"]["gui_descendants"])

    def test_exit_without_a_live_reaper_is_uncounted_and_never_subtracted(self) -> None:
        # The helper root and its git child both exited and a new helper took over:
        # nothing in the second reading reaped them, so their CPU is not guessed.
        gui = component(0.0, sample(100, 1, 0, 0.0, 0.0))
        before = Reading(gui, component(0.0, sample(500, 1, 0, 0.4, 0.0), sample(501, 500, 0, 0.7, 0.0)))
        after = Reading(component(2.0, sample(100, 1, 0, 0.0, 0.0)),
                        component(2.0, sample(600, 1, 5, 0.1, 0.0)))
        total = aggregate(before, after, 100, gui_rss_bytes=10)["total"]
        self.assertAlmostEqual(total["idle_cpu_percent"], 5.0)  # Only the new helper's 0.1 s.
        reason = total["incomplete_reasons"]["wsl_helper"]
        self.assertIn("[500, 501]", reason)
        self.assertIn("not counted", reason)
        self.assertFalse(total["complete"])

    def test_exit_through_an_exited_parent_is_credited_to_the_live_ancestor(self) -> None:
        # 101 (0.2 s before) ran under 150 (0.1 s before); both exited and the root reaped
        # 150 with its reaped children: 0.5 s + 0.3 s lifetimes, 0.8 s - 0.3 s seen = 0.5 s.
        before = Reading(component(0.0, sample(100, 1, 0, 1.0, 0.0), sample(150, 100, 0, 0.1, 0.0),
                                   sample(101, 150, 0, 0.2, 0.0)), None)
        after = Reading(component(2.0, sample(100, 1, 0, 1.0, 0.8)), None)
        total = aggregate(before, after, 100, gui_rss_bytes=10)["total"]
        self.assertAlmostEqual(total["idle_cpu_percent"], 100.0 * 0.5 / 2.0)

    def test_unattributable_exit_cpu_leaves_the_part_unmeasured_not_negative(self) -> None:
        # The child exited but its parent has not waited for it yet: nothing was added.
        before = Reading(component(0.0, sample(100, 1, 0, 1.0, 0.0), sample(101, 100, 0, 0.5, 0.0)), None)
        after = Reading(component(2.0, sample(100, 1, 0, 1.0, 0.0)), None)
        total = aggregate(before, after, 100, gui_rss_bytes=10)["total"]
        self.assertEqual(total["unavailable"], ["gui_descendants"])
        self.assertIn("could not be attributed", total["unavailable_reasons"]["gui_descendants"])
        self.assertFalse(total["complete"])

    def test_unmeasured_helper_nulls_every_total_and_keeps_the_partial_sum_labelled(self) -> None:
        before, after = self.readings()
        before = Reading(before.gui, missing("wsl.exe query produced no output"))
        result = aggregate(before, after, 100, gui_rss_bytes=1000)
        total = result["total"]
        self.assertEqual(total["unavailable"], ["wsl_helper"])
        self.assertIn("wsl.exe", total["unavailable_reasons"]["wsl_helper"])
        self.assertFalse(total["complete"])
        for field in ("rss_bytes", "process_count", "idle_cpu_percent"):
            self.assertIsNone(total[field], field)
        partial = total["partial_lower_bound"]
        self.assertEqual(partial["process_count"], 3)
        self.assertEqual(partial["rss_bytes"], 1000 + 40 + 10)
        self.assertAlmostEqual(partial["idle_cpu_percent"], 40.0)
        self.assertEqual(result["gui_only"]["unavailable"], [])
        self.assertEqual(result["gui_only"]["rss_bytes"], 1000)

    def test_complete_totals_carry_no_partial_block(self) -> None:
        reading = Reading(component(0.0, sample(100, 1, 0, 0.0)), None)
        after = Reading(component(1.0, sample(100, 1, 7, 0.1)), None)
        total = aggregate(reading, after, 100, gui_rss_bytes=10)["total"]
        self.assertNotIn("partial_lower_bound", total)
        self.assertTrue(total["complete"])
        self.assertEqual(total["incomplete_reasons"], {})

    def test_missing_gui_tree_leaves_cpu_unmeasured_not_zero(self) -> None:
        reading = Reading(missing("Windows process sampler unavailable"), None)
        result = aggregate(reading, reading, 100, gui_rss_bytes=1000)
        self.assertIsNone(result["gui_only"]["idle_cpu_percent"])
        self.assertEqual(result["gui_only"]["unavailable"], ["gui_cpu"])
        self.assertIsNone(result["total"]["idle_cpu_percent"])
        self.assertEqual(result["total"]["unavailable"], ["gui_cpu", "gui_descendants"])
        self.assertIsNone(result["total"]["rss_bytes"])
        self.assertIsNone(result["total"]["process_count"])
        self.assertEqual(result["total"]["partial_lower_bound"]["rss_bytes"], 1000)
        self.assertEqual(result["total"]["components"], ["gui", "gui_descendants"])

    def test_regressing_counter_and_unreadable_children_are_reported(self) -> None:
        before = Reading(component(0.0, sample(100, 1, 0, 2.0), sample(101, 100, 0, 1.0)), None)
        after = Reading(component(1.0, sample(100, 1, 0, 1.0), sample(101, 100, 5, 1.5),
                                  unreadable=[102]), None)
        result = aggregate(before, after, 100, gui_rss_bytes=10)
        self.assertIsNone(result["gui_only"]["idle_cpu_percent"])
        self.assertIn("went backwards", result["gui_only"]["unavailable_reasons"]["gui_cpu"])
        self.assertIn("unreadable", result["total"]["unavailable_reasons"]["gui_descendants"])
        self.assertFalse(result["total"]["complete"])
        self.assertIsNone(result["total"]["process_count"])
        self.assertIsNone(result["total"]["partial_lower_bound"]["idle_cpu_percent"])

    def test_zero_length_window_is_unavailable(self) -> None:
        reading = Reading(component(5.0, sample(100, 1, 0, 1.0)), None)
        result = aggregate(reading, reading, 100, gui_rss_bytes=10)
        self.assertIsNone(result["gui_only"]["idle_cpu_percent"])
        self.assertIn("duration", result["gui_only"]["unavailable_reasons"]["gui_cpu"])

    def test_helper_not_expected_is_not_listed_as_a_component(self) -> None:
        reading = Reading(component(0.0, sample(100, 1, 0, 0.0)), None)
        after = Reading(component(1.0, sample(100, 1, 0, 0.1)), None)
        total = aggregate(reading, after, 100, gui_rss_bytes=10)["total"]
        self.assertEqual(total["components"], ["gui", "gui_descendants"])
        self.assertTrue(total["complete"])
        self.assertEqual(total["rss_bytes"], 10)
        self.assertEqual(total["process_count"], 1)

    def test_helper_exits_are_incomplete_too(self) -> None:
        before, after = self.readings()
        after = Reading(after.gui, component(12.0, sample(500, 1, 300, 0.3)))  # 501 (git) exited
        total = aggregate(before, after, 100, gui_rss_bytes=1000)["total"]
        self.assertFalse(total["complete"])
        self.assertIn("501", total["incomplete_reasons"]["wsl_helper"])


class ProbeAndSourceTests(unittest.TestCase):
    def test_probe_reads_both_components_around_the_window(self) -> None:
        clock = iter([0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0])  # gui, helper, then gui, failing helper
        cpu = iter([1.0, 1.5])
        gui = lambda pid: TreeReading((sample(pid, 1, 100, next(cpu)),))
        helper = MagicMock(side_effect=[TreeReading((sample(500, 1, 10, 0.0),)),
                                        RuntimeError("sampler timed out")])
        probe = ResourceProbe(gui, helper, clock=lambda: next(clock))
        before = probe.begin(7)
        result = probe.finish(before, 7, gui_rss_bytes=500)
        self.assertAlmostEqual(result["gui_only"]["idle_cpu_percent"], 25.0)
        self.assertEqual(result["total"]["unavailable"], ["wsl_helper"])
        self.assertEqual(result["total"]["unavailable_reasons"]["wsl_helper"], "sampler timed out")

    def test_helper_tree_matches_repository_and_includes_git_children(self) -> None:
        processes = [
            LinuxProcess(sample(50, 1, 0, 0.0), ("/x/fastdiff", "--helper", "/repo")),
            LinuxProcess(sample(51, 50, 0, 0.0), ("git", "status")),
            LinuxProcess(sample(60, 1, 0, 0.0), ("/x/fastdiff", "--helper", "/other")),
            LinuxProcess(sample(70, 1, 0, 0.0), ("bash",)),
        ]
        reading = helper_tree_source(lambda: processes, "/repo")()
        self.assertEqual([s.pid for s in reading.processes], [50, 51])
        with self.assertRaises(ResourceUnavailable):
            helper_tree_source(lambda: processes, "/nowhere")()
        self.assertFalse(is_helper(("fastdiff", "--helper"), "/repo"))

    def test_wsl_table_is_read_with_an_argument_vector_and_constants_once(self) -> None:
        calls = []
        table = (b"==> /proc/uptime <==\n1000.50 3999.00\n"
                 b"\n==> /proc/50/cmdline <==\n/x/fastdiff\0--helper\0/repo\0\n"
                 b"\n==> /proc/50/stat <==\n" + stat_line(50, 1, 300, 0, 25, start=99000).encode()
                 + b"\n==> /proc/51/stat <==\n"
                 + stat_line(51, 50, 100, 0, 5, comm="git", cutime=50, start=99950).encode()
                 + b"\n==> /proc/51/cmdline <==\ngit\0status\0\n")

        def run(command):
            calls.append(command)
            if command[-2:] == ["getconf", "CLK_TCK"]:
                return b"100\n"
            if command[-2:] == ["getconf", "PAGESIZE"]:
                return b"4096\n"
            return table
        reader = wsl_table_reader("Ubuntu", run)
        first = reader()
        reader()
        self.assertEqual([entry.sample.pid for entry in first], [50, 51])
        self.assertEqual(first[0].args, ("/x/fastdiff", "--helper", "/repo"))
        self.assertEqual(first[0].sample.cpu_seconds, 3.0)
        self.assertEqual(first[0].sample.rss_bytes, 25 * 4096)
        self.assertEqual(first[1].sample.reaped_cpu_seconds, 0.5)
        # Ages come from /proc/uptime in the same read: 1000.5 - 990.0 and 1000.5 - 999.5.
        self.assertAlmostEqual(first[0].age_seconds, 10.5)
        self.assertAlmostEqual(first[1].age_seconds, 1.0)
        for command in calls:
            self.assertEqual(command[:5], ["wsl.exe", "--distribution", "Ubuntu", "--exec",
                                           command[4]])
            self.assertTrue(set(command).isdisjoint({"sh", "bash", "dash"}))
        self.assertEqual(sum(1 for c in calls if c[-1] == "PAGESIZE"), 1)
        self.assertIn("/proc/uptime", WSL_TABLE_COMMAND)
        with self.assertRaises(ResourceUnavailable):
            parse_wsl_table(b"nothing", TICKS, PAGE)

    def test_wsl_table_without_uptime_leaves_ages_unknown(self) -> None:
        table = (b"==> /proc/50/stat <==\n" + stat_line(50, 1, start=10).encode()
                 + b"\n==> /proc/50/cmdline <==\nx\0\n")
        self.assertIsNone(parse_wsl_table(table, TICKS, PAGE)[0].age_seconds)

    @unittest.skipIf(os.name == "nt", "Linux /proc layout")
    def test_local_table_reader_reads_uptime_from_the_proc_root(self) -> None:
        ticks, page = os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "uptime").write_text("500.00 900.00\n")
            (root / "60").mkdir()
            (root / "60" / "stat").write_text(stat_line(60, 1, start=100 * ticks))
            (root / "60" / "cmdline").write_bytes(b"dff\0--helper\0/repo\0")
            (entry,) = local_table_reader(root)()
        self.assertEqual(entry.args, ("dff", "--helper", "/repo"))
        self.assertAlmostEqual(entry.age_seconds, 400.0)
        self.assertEqual(page, os.sysconf("SC_PAGE_SIZE"))

    def test_a_helper_older_than_the_probe_is_not_the_guis_helper(self) -> None:
        def helper(pid, age, parent=1, args=("/x/fastdiff", "--helper", "/repo")):
            return LinuxProcess(sample(pid, parent, 0, 0.0), args, age)
        # 50 is the user's helper from yesterday; 80 and its git child started 3 s ago.
        table = [helper(50, 86_400.0), helper(80, 3.0),
                 helper(81, 2.0, parent=80, args=("git", "status")),
                 helper(90, 3.0, args=("/x/fastdiff", "--helper", "/other"))]
        reading = helper_tree_source(lambda: table, "/repo", elapsed=lambda: 5.0)()
        self.assertEqual([s.pid for s in reading.processes], [80, 81])
        # Within the tolerance of the harness clock, a helper is still accepted.
        borderline = [helper(70, 5.5)]
        self.assertEqual([s.pid for s in helper_tree_source(
            lambda: borderline, "/repo", elapsed=lambda: 5.0)().processes], [70])
        only_old = [helper(50, 86_400.0)]
        with self.assertRaises(ResourceUnavailable) as caught:
            helper_tree_source(lambda: only_old, "/repo", elapsed=lambda: 5.0)()
        self.assertIn("before", str(caught.exception))
        self.assertIn("[50]", str(caught.exception))

    def test_unknown_helper_start_time_is_unavailable_not_assumed_new(self) -> None:
        table = [LinuxProcess(sample(50, 1, 0, 0.0), ("/x/fastdiff", "--helper", "/repo"), None)]
        with self.assertRaises(ResourceUnavailable) as caught:
            helper_tree_source(lambda: table, "/repo", elapsed=lambda: 5.0)()
        self.assertIn("start time", str(caught.exception))

    def test_probe_for_ties_the_helper_filter_to_the_probe_creation_time(self) -> None:
        def helper(age):
            return (LinuxProcess(sample(50, 1, 0, 0.0), ("fastdiff", "--helper", "/repo"), age),)

        def probe(table):
            ticks = iter([100.0, 107.0])  # Created at 100 s; the helper is read at 107 s.
            with patch("benchmark_resources.wsl_table_reader", return_value=lambda: table):
                return probe_for(windows_gui=True, repository="/repo", sampler=MagicMock(),
                                 wsl_distro="Ubuntu", clock=lambda: next(ticks))
        with self.assertRaises(ResourceUnavailable):  # 400 s old: older than the whole run.
            probe(helper(400.0)).helper_tree()
        self.assertEqual([s.pid for s in probe(helper(6.0)).helper_tree().processes], [50])

    def test_windows_rows_become_a_tree_and_mark_unreadable_processes(self) -> None:
        sampler = MagicMock()
        sampler.tree.return_value = [(7, 1, 1000, 20_000_000), (8, 7, -1, 0),
                                     (9, 7, 500, 5_000_000)]
        reading = windows_tree_source(sampler)(7)
        self.assertEqual(reading.processes[0], ProcessSample(7, 1, 1000, 2.0))
        self.assertEqual(reading.unreadable_pids, (8,))
        sampler.tree.return_value = [(8, 7, 10, 1)]
        with self.assertRaises(ResourceUnavailable):
            windows_tree_source(sampler)(7)

    def test_windows_sampler_tree_uses_the_existing_persistent_process(self) -> None:
        process = MagicMock()
        process.stdout = io.StringIO("ready\n1024\n[[7,1,1000,20000000],[8,7,500,10000000]]\nnull\n")
        process.stdin = io.StringIO()
        with patch("benchmark_windows.subprocess.Popen", return_value=process) as spawn:
            sampler = WindowsSampler()
            self.assertEqual(sampler.query(7), (True, 1024))
            self.assertEqual(sampler.tree(7), [(7, 1, 1000, 20000000), (8, 7, 500, 10000000)])
            self.assertEqual(sampler.tree(7), [])
            self.assertEqual(process.stdin.getvalue(), "7\ntree 7\ntree 7\n")
            sampler.close()
            spawn.assert_called_once()
        with self.assertRaises(ValueError):
            parse_tree_response("[[1,2,3]]")
        with self.assertRaises(ValueError):
            parse_tree_response('{"pid": 1}')

    def test_sources_follow_the_platform_and_never_invent_a_helper(self) -> None:
        native = probe_for(windows_gui=False, repository="/repo")
        self.assertIsNone(native.helper_tree)
        windows_native = probe_for(windows_gui=True, repository="C:/r", sampler_error="no PowerShell")
        self.assertIsNone(windows_native.helper_tree)
        with self.assertRaises(ResourceUnavailable) as caught:
            windows_native.gui_tree(1)
        self.assertIn("no PowerShell", str(caught.exception))
        self.assertIsNotNone(probe_for(windows_gui=True, repository="/r", wsl_distro="Ubuntu",
                                       sampler=MagicMock()).helper_tree)
        self.assertIsNotNone(probe_for(windows_gui=True, repository="/r", windows_from_wsl=True,
                                       sampler=MagicMock()).helper_tree)
        with self.assertRaises(ResourceUnavailable):
            unavailable_tree("why")(1)


class SummaryTests(unittest.TestCase):
    def block(self, rss, cpu, total_unavailable=(), incomplete=None, count=3):
        gui = {"rss_bytes": rss, "idle_cpu_percent": cpu, "process_count": 1, "unavailable": []}
        failed = bool(total_unavailable)
        incomplete = incomplete or {}
        total = {"rss_bytes": None if failed else rss * 2,
                 "idle_cpu_percent": None if failed else cpu,
                 "process_count": None if failed else count,
                 "unavailable": list(total_unavailable), "incomplete_reasons": incomplete,
                 "complete": not failed and not incomplete}
        return {"gui_only": gui, "total": total}

    def test_unmeasured_runs_stay_visible(self) -> None:
        self.assertEqual(summarize_resources([None, None])["unavailable"], ["resources"])
        summary = summarize_resources([self.block(100, 1.0), self.block(200, None, ["wsl_helper"]),
                                       None])
        self.assertEqual(summary["runs"], 3)
        self.assertEqual(summary["measured_runs"], 2)
        self.assertEqual(summary["gui_only"]["max_rss_bytes"], 200)
        self.assertEqual(summary["gui_only"]["cpu_measured_runs"], 1)
        self.assertEqual(summary["total"]["unavailable"], ["wsl_helper"])
        self.assertEqual(summary["total"]["complete_runs"], 1)
        self.assertEqual(summary["total"]["measured_runs"], 2)

    def test_total_statistics_cover_complete_runs_only(self) -> None:
        blocks = [self.block(100, 1.0, count=3),
                  self.block(900, 80.0, incomplete={"gui_descendants": "exited"}, count=9),
                  self.block(500, 5.0, ["wsl_helper"], count=7)]
        total = summarize_resources(blocks)["total"]
        self.assertEqual(total["complete_runs"], 1)
        self.assertEqual(total["measured_runs"], 3)
        self.assertEqual(total["max_rss_bytes"], 200)
        self.assertEqual(total["max_process_count"], 3)
        self.assertEqual(total["p95_idle_cpu_percent"], 1.0)
        self.assertEqual(total["cpu_measured_runs"], 1)
        self.assertEqual(total["unavailable"], ["wsl_helper"])
        self.assertEqual(total["incomplete"], ["gui_descendants"])
        # The GUI process alone is still summarized over every measured run.
        self.assertEqual(summarize_resources(blocks)["gui_only"]["max_rss_bytes"], 900)

    def test_no_complete_run_gives_null_totals_not_zero(self) -> None:
        total = summarize_resources([self.block(100, None, ["gui_cpu"]),
                                     self.block(200, 3.0, incomplete={"wsl_helper": "exited"})]
                                    )["total"]
        self.assertEqual(total["complete_runs"], 0)
        self.assertEqual(total["measured_runs"], 2)
        for field in ("max_rss_bytes", "max_process_count", "p95_idle_cpu_percent"):
            self.assertIsNone(total[field], field)
        self.assertEqual(total["cpu_measured_runs"], 0)


if __name__ == "__main__":
    unittest.main()
