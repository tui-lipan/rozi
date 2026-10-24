//! POSIX media sources for opt-in benchmarks and consuming-media regression tests.
#![allow(unsafe_code)]

#[cfg(unix)]
mod posix {
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct SharedMemoryFixture {
        name: CString,
        owned: std::cell::Cell<bool>,
    }

    impl SharedMemoryFixture {
        pub fn supported() -> bool {
            true
        }

        pub fn cleanup(name: &[u8]) -> io::Result<()> {
            if name.first() != Some(&b'/') || name[1..].contains(&b'/') {
                return Err(io::Error::other("invalid fixture name"));
            }
            let source = Self {
                name: CString::new(name).map_err(io::Error::other)?,
                owned: std::cell::Cell::new(true),
            };
            drop(source);
            Ok(())
        }

        pub fn new(pixels: &[u8]) -> io::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let name = CString::new(format!(
                "/rz-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
            .map_err(io::Error::other)?;
            let source = Self {
                name,
                owned: std::cell::Cell::new(false),
            };
            source.create(pixels)?;
            Ok(source)
        }

        pub fn name(&self) -> &[u8] {
            self.name.as_bytes()
        }

        pub fn exists(&self) -> bool {
            // SAFETY: a valid POSIX name; an opened descriptor is immediately closed by OwnedFd.
            let fd = unsafe { libc::shm_open(self.name.as_ptr(), libc::O_RDONLY, 0) };
            if fd < 0 {
                return false;
            }
            drop(unsafe { OwnedFd::from_raw_fd(fd) });
            true
        }

        pub fn recreate(&self, pixels: &[u8]) -> io::Result<()> {
            self.create(pixels)
        }

        fn create(&self, pixels: &[u8]) -> io::Result<()> {
            let length = libc::off_t::try_from(pixels.len()).map_err(io::Error::other)?;
            // SAFETY: O_EXCL refuses existing objects; OwnedFd closes the resulting descriptor.
            let descriptor = unsafe {
                libc::shm_open(
                    self.name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                    0o600,
                )
            };
            if descriptor < 0 {
                return Err(io::Error::last_os_error());
            }
            self.owned.set(true);
            let _owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
            if unsafe { libc::ftruncate(descriptor, length) } < 0 {
                return Err(io::Error::last_os_error());
            }
            if pixels.is_empty() {
                return Ok(());
            }
            // Darwin permits mmap/fstat/ftruncate on shm descriptors, but not write/read/seek.
            let mapping = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    pixels.len(),
                    libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    descriptor,
                    0,
                )
            };
            if mapping == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the mapping covers exactly the copied range and is released after the copy.
            unsafe {
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapping.cast(), pixels.len());
                libc::munmap(mapping, pixels.len());
            }
            Ok(())
        }
    }

    impl Drop for SharedMemoryFixture {
        fn drop(&mut self) {
            // A consumer already unlinked a handed-over object. Do not unlink that name again.
            if self.owned.get() && self.exists() {
                unsafe { libc::shm_unlink(self.name.as_ptr()) };
            }
        }
    }
}

#[cfg(unix)]
pub use posix::SharedMemoryFixture;

#[cfg(not(unix))]
pub struct SharedMemoryFixture;

#[cfg(not(unix))]
impl SharedMemoryFixture {
    pub fn supported() -> bool {
        false
    }
    pub fn new(_pixels: &[u8]) -> std::io::Result<Self> {
        Err(std::io::Error::other("POSIX shared memory unavailable"))
    }
    pub fn name(&self) -> &[u8] {
        &[]
    }
    pub fn cleanup(_name: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
}
