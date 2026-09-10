//! Where this platform keeps `errno`.
//!
//! The signal handlers in [`super::cursor`] and [`super::server_lifecycle`] save and restore
//! `errno` around their writes, so they need its address, and every libc spells that accessor
//! differently. Both handlers used to carry their own `linux`/`not(linux)` split, which quietly
//! sent every non-Linux target to `__error` and broke the build on NetBSD (issue #3).
//!
//! So the mapping lives here once, and it is exhaustive on purpose: a target nobody has listed
//! fails with the `compile_error!` below, which names the four accessors, instead of picking up
//! whichever one happened to be in the fallback arm.

/// Address of the calling thread's `errno`.
///
/// # Safety
///
/// The pointer is valid for the calling thread only. Every accessor listed here is a thread-local
/// lookup, which is what lets the signal handlers call this and still be async-signal-safe.
#[cfg(any(target_os = "linux", target_os = "hurd"))]
pub(crate) unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

/// Address of the calling thread's `errno`. See the Linux arm for safety.
#[cfg(any(
    target_os = "android",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "emscripten"
))]
pub(crate) unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::__errno() }
}

/// Address of the calling thread's `errno`. See the Linux arm for safety.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
pub(crate) unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

/// Address of the calling thread's `errno`. See the Linux arm for safety.
#[cfg(any(target_os = "solaris", target_os = "illumos"))]
pub(crate) unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::___errno() }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "hurd",
    target_os = "android",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "emscripten",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "solaris",
    target_os = "illumos"
)))]
compile_error!(
    "rozi does not know how this target exposes `errno`. Add it to one of the arms in \
     src/platform/errno.rs: `__errno_location` on Linux and Hurd, `__errno` on Android and the \
     NetBSD-likes, `__error` on Apple, FreeBSD and DragonFly, `___errno` on Solaris and illumos."
);

#[cfg(test)]
mod tests {
    use super::errno_slot;

    /// Reading and writing through the pointer is what both signal handlers do, so a target that
    /// compiles but hands back a bogus address should fail here rather than in a handler.
    #[test]
    fn errno_slot_round_trips_a_value() {
        unsafe {
            let slot = errno_slot();
            let saved = *slot;
            *slot = libc::EINVAL;
            assert_eq!(*errno_slot(), libc::EINVAL);
            *slot = saved;
        }
    }
}
