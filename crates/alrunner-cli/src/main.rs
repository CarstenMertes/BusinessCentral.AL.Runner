//! `alrunner`: the user command line of AL Runner v3 (ADR-016). Today: `alrunner test`.

mod apps;
mod args;
mod capture;
mod coverage;
mod output;
mod run;
mod serve;
mod style;
mod testdata;

use args::Command;
use std::process::ExitCode;

#[cfg(all(feature = "mimalloc", not(target_family = "wasm")))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // An AL call nests native frames of the bytecode loop; give the runner thread room.
    let worker = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(real_main)
        .expect("spawn runner thread");
    let joined = worker.join();
    if std::env::var_os("AL_EXEC_REPORT").is_some() {
        eprintln!("{}", al_vm::exec_report());
    }
    match joined {
        Ok(code) => code,
        Err(_) => {
            eprintln!("error: the runner thread panicked (a bug)");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> ExitCode {
    let command = match args::parse(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => {
            print!("{}", args::HELP);
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("alrunner {} ({})", env!("CARGO_PKG_VERSION"), env!("ALRUNNER_COMMIT"));
            ExitCode::SUCCESS
        }
        Command::ListAssumptions(dir) => {
            let root = al_deps::cache::cache_root_from_env(dir.as_deref());
            print!("{}", al_testrun::flagged::list(&root));
            ExitCode::SUCCESS
        }
        Command::CacheClear(dir) => {
            let folder =
                al_testrun::compile_cache::CompileCache::under(&al_deps::cache::cache_root_from_env(dir.as_deref()));
            let (files, bytes) = al_testrun::compile_cache::clear(&folder);
            println!(
                "compile cache {}: removed {files} file(s), {:.1} MB",
                folder.display(),
                bytes as f64 / 1e6
            );
            ExitCode::SUCCESS
        }
        Command::Warm(a) => match run::warm(&a) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        },
        Command::Serve(daemon) => match serve::serve(&daemon) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        },
        Command::Test(a) => match run::test(&a) {
            Ok(code) => ExitCode::from(code),
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        },
    }
}
