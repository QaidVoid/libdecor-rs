//! Anonymous shared-memory buffer helper.
//!
//! [`ShmBuffer`] wraps a `memfd`/POSIX shared-memory file descriptor and
//! the corresponding `mmap` of its contents. Callers use it to back
//! `wl_shm_pool`-derived `wl_buffer` objects: get the file descriptor
//! with [`ShmBuffer::as_fd`], hand it to `wl_shm.create_pool`, then
//! write pixels through [`ShmBuffer::as_mut_slice`].
//!
//! The mapping is unmapped when `ShmBuffer` is dropped; the underlying
//! file descriptor closes with it.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::ptr::NonNull;
use std::slice;
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use rustix::shm::{self, Mode, OFlags};

use crate::error::Result;

/// A mmap-backed shared-memory region suitable for `wl_shm` buffer
/// allocation.
pub struct ShmBuffer {
    fd: OwnedFd,
    ptr: NonNull<u8>,
    len: usize,
}

unsafe impl Send for ShmBuffer {}

impl ShmBuffer {
    /// Allocate a new shared-memory region of `len` bytes.
    pub fn new(len: usize) -> Result<Self> {
        let name = unique_name();
        let fd = shm::open(
            name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL,
            Mode::RUSR | Mode::WUSR,
        )?;
        let _ = shm::unlink(name.as_str());
        rustix::fs::ftruncate(&fd, len as u64)?;

        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )
        }?;
        let ptr = NonNull::new(ptr.cast::<u8>()).expect("mmap returned null");

        Ok(Self { fd, ptr, len })
    }

    /// Borrow the file descriptor (for passing to `wl_shm.create_pool`).
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Region size in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when the buffer has zero length.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Borrow the mapped memory as a byte slice.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Borrow the mapped memory as a mutable byte slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Borrow the mapped memory as a slice of 32-bit pixels.
    ///
    /// `len` must be a multiple of four; if not, the trailing bytes are
    /// ignored.
    pub fn as_pixels(&mut self) -> &mut [u32] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr().cast::<u32>(), self.len / 4) }
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(self.ptr.as_ptr().cast(), self.len);
        }
    }
}

fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("/libdecor-rs-{}-{}", std::process::id(), n)
}
