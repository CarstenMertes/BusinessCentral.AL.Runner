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

/// The CPU time (user and system) of the child processes this process has waited for, in nanoseconds. The cost of a
/// run that is a child process: unlike the process clock it does not move with the other tests of the same binary.
#[cfg(target_os = "linux")]
pub fn children_cpu_ns() -> u64 {
    // SAFETY: an all-zero `rusage` is a valid value, and the call fills it in.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `ru` is a valid rusage for the call to fill in.
    unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1_000;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

#[cfg(not(target_os = "linux"))]
pub fn children_cpu_ns() -> u64 {
    0
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

/// What a perf budget does with a measurement (owner decision 2026-10-20): where the hardware counter exists the
/// instruction budget is enforced; the CPU-time fallback (a CI runner has no counter) only warns.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

pub fn decide(counting: bool, over_limit: bool) -> Verdict {
    match (over_limit, counting) {
        (false, _) => Verdict::Pass,
        (true, true) => Verdict::Fail,
        (true, false) => Verdict::Warn,
    }
}

/// Prints which mode the budget runs in; every perf budget calls it once per run.
pub fn print_mode(counting: bool) {
    if counting {
        eprintln!("mode: instructions (enforced)");
    } else {
        eprintln!("mode: CPU-time fallback (warning only)");
    }
}

/// Applies the verdict: a failure panics with `text`, a warning prints a GitHub Actions warning line.
pub fn enforce(counting: bool, over_limit: bool, text: &str) {
    match decide(counting, over_limit) {
        Verdict::Pass => {}
        Verdict::Warn => {
            // Workflow commands end at a newline; keep the text on one line.
            let one_line = text.lines().next().unwrap_or("");
            eprintln!("::warning title=perf budget (CPU fallback, not enforced)::{one_line}");
            eprintln!("{text}");
        }
        Verdict::Fail => panic!("{text}"),
    }
}

#[cfg(test)]
mod decide_tests {
    use super::*;

    #[test]
    fn counter_mode_over_the_limit_fails() {
        assert_eq!(decide(true, true), Verdict::Fail);
        assert_eq!(decide(true, false), Verdict::Pass);
    }

    #[test]
    fn cpu_fallback_over_the_limit_only_warns() {
        assert_eq!(decide(false, true), Verdict::Warn);
        assert_eq!(decide(false, false), Verdict::Pass);
    }
}
