//! The CPU time of one thread, readable from another thread. The test watchdog (runtime.rs)
//! reads the clock of the thread that runs the test. One implementation per OS family:
//! POSIX thread CPU clocks (Linux, BSD), Mach `thread_info` (macOS), `GetThreadTimes` (Windows).
//! Anywhere else `ThreadClock::current` returns `None` and the watchdog counts wall time.

use std::time::Duration;

/// A handle on the CPU clock of the thread that created it. Valid while that thread lives.
pub struct ThreadClock(imp::Raw);

// SAFETY: the raw value is a clock id, a Mach port name or an owned thread handle; each can be
// read from any thread.
unsafe impl Send for ThreadClock {}
unsafe impl Sync for ThreadClock {}

impl ThreadClock {
    /// The clock of the calling thread.
    pub fn current() -> Option<ThreadClock> {
        imp::current().map(ThreadClock)
    }

    /// The CPU time (user and system) the thread has used since it started.
    pub fn read(&self) -> Option<Duration> {
        imp::read(&self.0)
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
mod imp {
    use std::time::Duration;

    pub type Raw = libc::clockid_t;

    pub fn current() -> Option<Raw> {
        let mut id: libc::clockid_t = 0;
        // SAFETY: `id` is a valid out pointer; the thread is the calling one.
        let rc = unsafe { libc::pthread_getcpuclockid(libc::pthread_self(), &mut id) };
        (rc == 0).then_some(id)
    }

    pub fn read(id: &Raw) -> Option<Duration> {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `ts` is a valid out pointer.
        let rc = unsafe { libc::clock_gettime(*id, &mut ts) };
        (rc == 0).then(|| Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32))
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use std::time::Duration;

    pub type Raw = libc::mach_port_t;

    pub fn current() -> Option<Raw> {
        // SAFETY: the calling thread's pthread has a Mach thread port for as long as it lives.
        let port = unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) };
        (port != 0).then_some(port)
    }

    pub fn read(port: &Raw) -> Option<Duration> {
        // SAFETY: an all-zero thread_basic_info is a valid value (plain integers).
        let mut info: libc::thread_basic_info = unsafe { std::mem::zeroed() };
        let mut count = libc::THREAD_BASIC_INFO_COUNT;
        // SAFETY: `info` is as large as `count` words; both are valid out pointers.
        let rc = unsafe {
            libc::thread_info(
                *port,
                libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
                (&mut info as *mut libc::thread_basic_info).cast::<libc::integer_t>(),
                &mut count,
            )
        };
        if rc != 0 {
            return None;
        }
        let micros = |t: libc::time_value_t| t.seconds.max(0) as u64 * 1_000_000 + t.microseconds.max(0) as u64;
        Some(Duration::from_micros(micros(info.user_time) + micros(info.system_time)))
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::time::Duration;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn DuplicateHandle(
            source_process: *mut c_void,
            source: *mut c_void,
            target_process: *mut c_void,
            target: *mut *mut c_void,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn GetThreadTimes(
            thread: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const DUPLICATE_SAME_ACCESS: u32 = 2;

    /// A real handle to the thread (the pseudo handle from `GetCurrentThread` means "the caller"
    /// in whichever thread uses it). Closed when dropped.
    pub struct Raw(*mut c_void);

    impl Drop for Raw {
        fn drop(&mut self) {
            // SAFETY: the handle was made by `DuplicateHandle` and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn current() -> Option<Raw> {
        let mut real: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid pseudo handles in, valid out pointer.
        let ok = unsafe {
            let process = GetCurrentProcess();
            DuplicateHandle(
                process,
                GetCurrentThread(),
                process,
                &mut real,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        (ok != 0 && !real.is_null()).then_some(Raw(real))
    }

    pub fn read(h: &Raw) -> Option<Duration> {
        let (mut c, mut e, mut k, mut u) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        // SAFETY: valid handle and out pointers.
        let ok = unsafe { GetThreadTimes(h.0, &mut c, &mut e, &mut k, &mut u) };
        if ok == 0 {
            return None;
        }
        let ticks = |t: FileTime| (u64::from(t.high) << 32) | u64::from(t.low);
        // FILETIME counts 100 ns.
        Some(Duration::from_nanos((ticks(k) + ticks(u)) * 100))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::time::Duration;

    pub type Raw = ();

    pub fn current() -> Option<Raw> {
        None
    }

    pub fn read(_: &Raw) -> Option<Duration> {
        None
    }
}
