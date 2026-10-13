//! Assign suspended children to a job before any descendant can escape ownership.
use std::{io, os::windows::io::AsRawHandle, process::Child};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns a valid handle, and closes it exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct WindowsJob(OwnedHandle);

impl WindowsJob {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: null attributes/name request an unnamed, non-inheritable job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(OwnedHandle(handle));
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the live job handle and pointer describe the matching Win32
        // structure; its size is passed explicitly and it outlives the call.
        let success = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(super) fn attach_and_resume(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are live; the process was created suspended by
        // run(), so it cannot spawn descendants before assignment completes.
        if unsafe { AssignProcessToJobObject(self.0.0, child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_owned_thread(child.id())
    }

    pub(super) fn terminate(&self) {
        // SAFETY: this owned job contains only our child and its descendants.
        // Termination is idempotent, and closing the handle repeats the guarantee.
        unsafe {
            TerminateJobObject(self.0.0, 1);
        }
    }
}

fn resume_owned_thread(pid: u32) -> io::Result<()> {
    // SAFETY: requesting a read-only thread snapshot needs no pointer arguments.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = OwnedHandle(raw);
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry is writable and initialized with the required structure size.
    let mut found = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
    while found {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the thread ID came from the snapshot and belongs to our
            // suspended child; request only permission to resume that thread.
            let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let thread = OwnedHandle(raw);
            // SAFETY: thread is a live, owned handle with resume permission.
            let previous = unsafe { ResumeThread(thread.0) };
            if previous == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            if previous > 0 {
                return Ok(());
            }
        }
        // SAFETY: the snapshot remains open and entry retains its required size.
        found = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Cannot locate the suspended Git thread",
    ))
}
