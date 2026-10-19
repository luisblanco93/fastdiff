//! Returns memory freed while preparing a large document to the operating system.

/// Smaller documents free too little for a trim to be worth its walk over the arenas.
pub(super) const TRIM_THRESHOLD_BYTES: usize = 1024 * 1024;

/// Preparing a document frees its multi-megabyte source buffer inside the worker thread's glibc
/// arena, which keeps those pages resident; each open document tab then costs that much extra.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(super) fn release_freed_memory() {
    // SAFETY: `malloc_trim` takes no pointers and may be called from any thread.
    unsafe { libc::malloc_trim(0) };
}

/// Other allocators return large freed blocks themselves.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(super) fn release_freed_memory() {}
