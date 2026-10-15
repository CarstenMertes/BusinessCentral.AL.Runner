//! A counter of the instructions a piece of code executes (`perf_event_open`), with the CPU time as the fallback where
//! the kernel refuses it. Shared by the perf budgets (`perf_vm.rs`, `perf_budget.rs`): `#[path]` includes this file.
//!
//! Instructions do not move with the load of the machine, so a budget on them can be tight and still not flap.
//! `AL_VM_BENCH_CPU_TIME=1` forces the fallback.

#![allow(dead_code)]

/// A counter of the user-space instructions of the calling thread (and, with `inherit`, of the threads it starts), or the
/// CPU time of the thread (of the process with `inherit`) where the kernel refuses.
pub enum Meter {
    Instructions(i32),
    CpuTime(bool),
}

#[cfg(target_os = "linux")]
impl Meter {
    /// `inherit`: threads the measured code starts count too (open the meter before they are started).
    pub fn new(inherit: bool) -> Meter {
        // struct perf_event_attr, size 112 (PERF_ATTR_SIZE_VER5), as words: type | size, config, sample_period,
        // sample_type, read_format, flags (disabled = bit 0, exclude_kernel = bit 5, exclude_hv = bit 6).
        const PERF_TYPE_HARDWARE: u64 = 0;
        const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
        let mut attr = [0u64; 14];
        attr[0] = PERF_TYPE_HARDWARE | (112u64 << 32);
        attr[1] = PERF_COUNT_HW_INSTRUCTIONS;
        attr[5] = 1 | (1 << 5) | (1 << 6) | (u64::from(inherit) << 1);
        // SAFETY: `attr` is a valid, fully initialised perf_event_attr of the size it declares.
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0) };
        if fd >= 0 && std::env::var_os("AL_VM_BENCH_CPU_TIME").is_none() {
            Meter::Instructions(fd as i32)
        } else {
            Meter::CpuTime(inherit)
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Meter::Instructions(_) => "instructions",
            Meter::CpuTime(false) => "thread CPU time (ns)",
            Meter::CpuTime(true) => "process CPU time (ns)",
        }
    }

    /// Runs `f` and returns what it cost.
    pub fn measure<T>(&self, f: impl FnOnce() -> T) -> (T, u64) {
        const ENABLE: libc::c_ulong = 0x2400;
        const DISABLE: libc::c_ulong = 0x2401;
        match self {
            Meter::Instructions(fd) => {
                // The counter of a thread that was started under an inherited meter keeps its count when the meter is
                // reset, so the cost is a difference of two readings.
                let read = || {
                    let mut count = 0u64;
                    // SAFETY: `fd` is a perf event descriptor that this value owns; `count` is 8 bytes, the size of one
                    // counter value.
                    unsafe { libc::read(*fd, (&mut count as *mut u64).cast(), 8) };
                    count
                };
                // SAFETY: the ioctls take no argument.
                unsafe { libc::ioctl(*fd, ENABLE, 0) };
                let before = read();
                let out = f();
                let after = read();
                // SAFETY: as above.
                unsafe { libc::ioctl(*fd, DISABLE, 0) };
                (out, after - before)
            }
            Meter::CpuTime(process) => {
                let now = || if *process { process_cpu_ns() } else { thread_cpu_ns() };
                let t = now();
                let out = f();
                (out, now() - t)
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub fn process_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid timespec for the call to fill in.
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(target_os = "linux")]
pub fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid timespec for the call to fill in.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(not(target_os = "linux"))]
impl Meter {
    pub fn new(inherit: bool) -> Meter {
        Meter::CpuTime(inherit)
    }
    pub fn name(&self) -> &'static str {
        "thread CPU time (ns)"
    }
    pub fn measure<T>(&self, f: impl FnOnce() -> T) -> (T, u64) {
        let t = std::time::Instant::now();
        let out = f();
        (out, t.elapsed().as_nanos() as u64)
    }
}
