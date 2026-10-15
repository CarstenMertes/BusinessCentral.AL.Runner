//! The VM speed budget (ADR-031): instructions per iteration of six small programs that run in the interpreter.
//!
//! The programs are the test codeunits of `tests/vm-bench` (our own AL code): an Integer loop, a Decimal loop, a procedure
//! call, text builtins, math builtins and record operations. Each runs on one thread; the test counts the user-space
//! instructions that thread executes while the tests of the codeunit run (`perf_event_open`), subtracts what an empty test
//! codeunit costs (the session and the engine), and divides by the iterations. Instructions, not seconds: they do not move
//! with the load of the machine, so the budget can be tight (10 % over the value of the day it was set) and still not flap.
//!
//! Where the kernel does not allow the counter (a virtual machine without a PMU, `perf_event_paranoid` above 2) the test
//! falls back to the CPU time of the thread, with limits three times as wide, and says so. A fast path that merges lowers
//! its limit in the same commit. `AL_VM_BENCH_CPU_TIME=1` forces the fallback (to set or check its limits).
//!
//! Ignored by default (a release build is needed):
//!
//! ```text
//! cargo test --release -p al-test-cli --test perf_vm -- --ignored --nocapture
//! ```

use al_sema::AppKind;
use al_test_cli::runtime;
use al_testrun::Outcome;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use std::path::{Path, PathBuf};

/// One benchmark: the test codeunit, the iterations of its loop (the unit of the limits; the number in the AL file), the
/// limit in instructions per iteration, and the wall budget in CPU nanoseconds per iteration for the fallback.
struct Bench {
    name: &'static str,
    codeunit: u32,
    iterations: u64,
    limit_instructions: u64,
    limit_cpu_ns: u64,
}

/// Limits: the measured value of the day plus 10 % (instructions), and three times the CPU time of this machine (fallback).
/// Each fast path of ADR-031 that merges lowers them.
const BENCHES: &[Bench] = &[
    Bench {
        name: "IntLoop",
        codeunit: 70002,
        iterations: 300_000,
        limit_instructions: 1140,
        limit_cpu_ns: 1000,
    },
    Bench {
        name: "DecimalLoop",
        codeunit: 70003,
        iterations: 100_000,
        limit_instructions: 2180,
        limit_cpu_ns: 1000,
    },
    Bench {
        name: "CallLoop",
        codeunit: 70004,
        iterations: 100_000,
        limit_instructions: 3080,
        limit_cpu_ns: 2300,
    },
    Bench {
        name: "TextLoop",
        codeunit: 70005,
        iterations: 30_000,
        limit_instructions: 7700,
        limit_cpu_ns: 4400,
    },
    Bench {
        name: "BuiltinLoop",
        codeunit: 70006,
        iterations: 50_000,
        limit_instructions: 7020,
        limit_cpu_ns: 6000,
    },
    Bench {
        name: "RecordLoop",
        codeunit: 70007,
        iterations: 15_000,
        limit_instructions: 17820,
        limit_cpu_ns: 9500,
    },
];
const EMPTY: u32 = 70001;
/// Runs per benchmark; the least counts (a run that was disturbed counts more, never less).
const RUNS: usize = 3;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A counter of the user-space instructions of the calling thread, or the CPU time of it where the kernel refuses.
enum Meter {
    Instructions(i32),
    CpuTime,
}

#[cfg(target_os = "linux")]
impl Meter {
    fn new() -> Meter {
        // struct perf_event_attr, size 112 (PERF_ATTR_SIZE_VER5), as words: type | size, config, sample_period,
        // sample_type, read_format, flags (disabled = bit 0, exclude_kernel = bit 5, exclude_hv = bit 6).
        const PERF_TYPE_HARDWARE: u64 = 0;
        const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
        let mut attr = [0u64; 14];
        attr[0] = PERF_TYPE_HARDWARE | (112u64 << 32);
        attr[1] = PERF_COUNT_HW_INSTRUCTIONS;
        attr[5] = 1 | (1 << 5) | (1 << 6);
        // SAFETY: `attr` is a valid, fully initialised perf_event_attr of the size it declares.
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0) };
        if fd >= 0 && std::env::var_os("AL_VM_BENCH_CPU_TIME").is_none() {
            Meter::Instructions(fd as i32)
        } else {
            Meter::CpuTime
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Meter::Instructions(_) => "instructions",
            Meter::CpuTime => "thread CPU time (ns)",
        }
    }

    /// Runs `f` and returns what it cost.
    fn measure<T>(&self, f: impl FnOnce() -> T) -> (T, u64) {
        const ENABLE: libc::c_ulong = 0x2400;
        const DISABLE: libc::c_ulong = 0x2401;
        const RESET: libc::c_ulong = 0x2403;
        match self {
            Meter::Instructions(fd) => {
                // SAFETY: `fd` is a perf event descriptor that this value owns; the ioctls take no argument.
                unsafe {
                    libc::ioctl(*fd, RESET, 0);
                    libc::ioctl(*fd, ENABLE, 0);
                }
                let out = f();
                let mut count = 0u64;
                // SAFETY: as above; `count` is 8 bytes, the size of one counter value.
                unsafe {
                    libc::ioctl(*fd, DISABLE, 0);
                    libc::read(*fd, (&mut count as *mut u64).cast(), 8);
                }
                (out, count)
            }
            Meter::CpuTime => {
                let t = thread_cpu_ns();
                let out = f();
                (out, thread_cpu_ns() - t)
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid timespec for the call to fill in.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(not(target_os = "linux"))]
impl Meter {
    fn new() -> Meter {
        Meter::CpuTime
    }
    fn name(&self) -> &'static str {
        "thread CPU time (ns)"
    }
    fn measure<T>(&self, f: impl FnOnce() -> T) -> (T, u64) {
        let t = std::time::Instant::now();
        let out = f();
        (out, t.elapsed().as_nanos() as u64)
    }
}

/// The cost of running the tests of `codeunit` (compile excluded) and whether they passed.
fn cost(meter: &Meter, codeunit: u32) -> (u64, bool) {
    let app = runtime::load_app(&root().join("crates/al-test-cli/tests/vm-bench")).unwrap();
    let manifest = runtime::load_manifest(&root().join("crates/al-test-cli/tests/vm-bench")).unwrap();
    let opts = RunOptions {
        only: Some(vec![(0, codeunit)]),
        ..Default::default()
    };
    let compiled = CorpusRuntime::compile_suites(
        vec![app],
        vec![AppKind::Workspace],
        &[manifest],
        &Default::default(),
        None,
        &[SuiteRun::everything()],
        &opts,
    )
    .unwrap();
    // A single test codeunit runs on the calling thread, which is the one the meter watches.
    let (rt, count) = meter.measure(|| CorpusRuntime::run_compiled(&compiled, None, Default::default(), &opts));
    let passed = !rt.results().is_empty() && rt.results().iter().all(|r| r.outcome == Outcome::Pass);
    (count, passed)
}

fn least(meter: &Meter, codeunit: u32) -> u64 {
    (0..RUNS)
        .map(|_| {
            let (count, passed) = cost(meter, codeunit);
            assert!(passed, "the benchmark codeunit {codeunit} did not pass");
            count
        })
        .min()
        .unwrap()
}

#[test]
#[ignore = "perf budget: run with `cargo test --release -p al-test-cli --test perf_vm -- --ignored --nocapture`"]
fn the_interpreter_stays_within_its_instruction_budget() {
    if cfg!(debug_assertions) {
        panic!("perf budgets are only meaningful in --release; this is a debug build");
    }
    let meter = Meter::new();
    let counting = matches!(meter, Meter::Instructions(_));
    eprintln!("vm budget: counting {}", meter.name());
    let empty = least(&meter, EMPTY);
    let mut over = Vec::new();
    for b in BENCHES {
        let total = least(&meter, b.codeunit);
        let per = total.saturating_sub(empty) / b.iterations;
        let limit = if counting { b.limit_instructions } else { b.limit_cpu_ns };
        eprintln!(
            "vm budget: {:<12} {:>8} per iteration (limit {:>8}), {} for {} iterations, {} for an empty codeunit",
            b.name, per, limit, total, b.iterations, empty
        );
        if limit != 0 && per > limit {
            over.push(format!("{} used {per} per iteration, limit {limit}", b.name));
        }
    }
    assert!(
        over.is_empty(),
        "over the VM budget ({}): {}",
        meter.name(),
        over.join("; ")
    );
}
