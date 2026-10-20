//! Read-only memory mapping of a model file (phase M2).
//!
//! The weights are not copied into the process: the operating system pages
//! them in from the disk as they are read and can drop them again under
//! memory pressure, so a model costs only what is actually in use.

use std::fs::File;
use std::io;

pub struct Mmap {
    ptr: *const u8,
    len: usize,
}

// SAFETY: the mapping is read-only and lives until `Drop`; sharing the
// pointer between threads only ever reads.
unsafe impl Send for Mmap {}
unsafe impl Sync for Mmap {}

impl Mmap {
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` maps `len` readable bytes until `self` is dropped.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Maps the whole file. Empty files cannot be mapped; the caller reads
    /// those normally.
    #[cfg(unix)]
    pub fn map(file: &File) -> io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| io::Error::other("file too large to map"))?;
        if len == 0 {
            return Err(io::Error::other("empty file"));
        }
        // SAFETY: a fresh private read-only mapping of an open file.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr as *const u8,
            len,
        })
    }

    #[cfg(windows)]
    pub fn map(file: &File) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY,
        };
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| io::Error::other("file too large to map"))?;
        if len == 0 {
            return Err(io::Error::other("empty file"));
        }
        // SAFETY: plain Win32 calls on an open file handle; the mapping
        // handle can be closed once the view exists.
        unsafe {
            let mapping = CreateFileMappingW(
                file.as_raw_handle() as _,
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            );
            if mapping.is_null() {
                return Err(io::Error::last_os_error());
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            let err = io::Error::last_os_error();
            CloseHandle(mapping);
            if view.Value.is_null() {
                return Err(err);
            }
            Ok(Self {
                ptr: view.Value as *const u8,
                len,
            })
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn map(_file: &File) -> io::Result<Self> {
        Err(io::Error::other("memory mapping is not available here"))
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: unmaps exactly what `map` mapped.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
        #[cfg(windows)]
        // SAFETY: unmaps exactly the view `map` created.
        unsafe {
            use windows_sys::Win32::System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS};
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.ptr as *mut _,
            });
        }
    }
}
