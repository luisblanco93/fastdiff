"""Windows process queries for the benchmark drivers (import-safe on Linux)."""

from __future__ import annotations

import ctypes
import json
import queue
import subprocess
import threading

TREE_PREFIX = "tree "
SAMPLER_STARTUP_SECONDS = 30
SAMPLER_QUERY_SECONDS = 10
SAMPLER_TREE_SECONDS = 30

# One CIM snapshot gives parent links, working set, and kernel+user time (100 ns
# units). A child only joins the tree when it was created after its parent, so a
# stale parent id that was later reused cannot pull in unrelated processes.
# Each row is [pid,parent,working_set_or_-1,cpu_100ns].
TREE_SCRIPT = (
    "try { "
    "$all = @(Get-CimInstance -ClassName Win32_Process -Property "
    "ProcessId,ParentProcessId,WorkingSetSize,KernelModeTime,UserModeTime,CreationDate "
    "-ErrorAction Stop); "
    "$byId = @{}; foreach ($p in $all) { $byId[[int]$p.ProcessId] = $p }; "
    "$keep = @{}; $keep[$requestedId] = $true; $grew = $true; "
    "while ($grew) { $grew = $false; foreach ($p in $all) { "
    "$id = [int]$p.ProcessId; $parent = [int]$p.ParentProcessId; "
    "if (-not $keep.ContainsKey($id) -and $keep.ContainsKey($parent) "
    "-and $byId.ContainsKey($parent) "
    "-and $p.CreationDate -ge $byId[$parent].CreationDate) { "
    "$keep[$id] = $true; $grew = $true } } }; "
    "$rows = @(); foreach ($p in $all) { if ($keep.ContainsKey([int]$p.ProcessId)) { "
    "$ws = -1; if ($null -ne $p.WorkingSetSize) { $ws = [int64]$p.WorkingSetSize }; "
    "$cpu = [int64]$p.KernelModeTime + [int64]$p.UserModeTime; "
    "$rows += ('[{0},{1},{2},{3}]' -f [int]$p.ProcessId, [int]$p.ParentProcessId, $ws, $cpu) } }; "
    "if ($rows.Count -eq 0) { [Console]::WriteLine('null') } "
    "else { [Console]::WriteLine('[' + ($rows -join ',') + ']') } "
    "} catch { [Console]::WriteLine('null') }; "
)


def windows_process_info(pid: int) -> tuple[bool, int]:
    from ctypes import wintypes

    class MemoryCounters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD), ("faults", wintypes.DWORD),
            ("peak_rss", ctypes.c_size_t), ("rss", ctypes.c_size_t),
            ("peak_paged", ctypes.c_size_t), ("paged", ctypes.c_size_t),
            ("peak_nonpaged", ctypes.c_size_t), ("nonpaged", ctypes.c_size_t),
            ("pagefile", ctypes.c_size_t), ("peak_pagefile", ctypes.c_size_t),
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    psapi.GetProcessMemoryInfo.argtypes = [
        wintypes.HANDLE, ctypes.POINTER(MemoryCounters), wintypes.DWORD,
    ]
    handle = kernel.OpenProcess(0x1000 | 0x100000, False, pid)
    if not handle:
        if ctypes.get_last_error() == 87:  # Process no longer exists.
            return False, 0
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if kernel.WaitForSingleObject(handle, 0) == 0:
            return False, 0
        counters = MemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return True, counters.rss
    finally:
        kernel.CloseHandle(handle)


def parse_tree_response(line: str) -> list[tuple[int, int, int, int]]:
    """Decode one `tree` reply: rows of (pid, parent, working set or -1, cpu 100 ns)."""
    rows = json.loads(line)
    if rows is None:
        return []
    if not isinstance(rows, list):
        raise ValueError("Windows process tree query returned invalid data")
    parsed: list[tuple[int, int, int, int]] = []
    for row in rows:
        if (not isinstance(row, list) or len(row) != 4
                or any(type(item) is not int for item in row)
                or row[0] <= 0 or row[1] < 0 or row[2] < -1 or row[3] < 0):
            raise ValueError("Windows process tree query returned invalid data")
        parsed.append((row[0], row[1], row[2], row[3]))
    return parsed


class WindowsSampler:
    """Keep one owned Windows process for RSS/tree queries instead of launching per poll."""

    def __init__(self) -> None:
        command = (
            "[Console]::WriteLine('ready'); "
            "while ($null -ne ($line = [Console]::ReadLine())) { "
            "$mode = 'rss'; $text = $line; "
            f"if ($line.StartsWith('{TREE_PREFIX}')) {{ $mode = 'tree'; "
            f"$text = $line.Substring({len(TREE_PREFIX)}) }}; "
            "$requestedId = 0; "
            "if (![int]::TryParse($text, [ref]$requestedId) -or $requestedId -le 0) { "
            "[Console]::WriteLine('null'); continue }; "
            f"if ($mode -eq 'tree') {{ {TREE_SCRIPT} continue }}; "
            "$process = Get-Process -Id $requestedId -ErrorAction SilentlyContinue; "
            "if ($null -eq $process) { [Console]::WriteLine('null') } "
            "else { try { $rss = $process.WorkingSet64; "
            "if ($rss -gt 0) { [Console]::WriteLine([string]$rss) } "
            "else { [Console]::WriteLine('null') } } "
            "catch { [Console]::WriteLine('null') } } }"
        )
        self.process = subprocess.Popen(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, encoding="ascii",
        )
        self.responses: queue.Queue[str] = queue.Queue(maxsize=4)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        try:
            if self.responses.get(timeout=SAMPLER_STARTUP_SECONDS).strip() != "ready":
                raise RuntimeError("Windows memory sampler did not become ready")
        except (queue.Empty, RuntimeError):
            self.close()
            raise RuntimeError("Windows memory sampler startup failed") from None

    def _read(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            try:
                self.responses.put(line, timeout=1)
            except queue.Full:
                return

    def _exchange(self, request: str, timeout: float, what: str) -> str:
        assert self.process.stdin is not None
        self.process.stdin.write(f"{request}\n")
        self.process.stdin.flush()
        try:
            return self.responses.get(timeout=timeout)
        except queue.Empty:
            raise RuntimeError(f"Windows {what} query timed out") from None

    def query(self, pid: int) -> tuple[bool, int]:
        if type(pid) is not int or pid <= 0:
            raise ValueError("invalid Windows PID")
        rss = json.loads(self._exchange(str(pid), SAMPLER_QUERY_SECONDS, "memory sampler"))
        if rss is None:
            return False, 0
        if type(rss) is not int or rss <= 0:
            raise ValueError("Windows resident memory query returned invalid data")
        return True, rss

    def tree(self, pid: int) -> list[tuple[int, int, int, int]]:
        """Process `pid` and its descendants; an empty list means it is gone."""
        if type(pid) is not int or pid <= 0:
            raise ValueError("invalid Windows PID")
        return parse_tree_response(
            self._exchange(f"{TREE_PREFIX}{pid}", SAMPLER_TREE_SECONDS, "process tree"))

    def close(self) -> None:
        if self.process.stdin is not None:
            try:
                self.process.stdin.close()
            except (BrokenPipeError, OSError):
                pass  # A closed peer must still be reaped below.
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.reader.join(timeout=2)
        if self.process.stdout is not None:
            self.process.stdout.close()
