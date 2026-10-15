//! `alrunner`: the user command line of AL Runner v3 (ADR-016). Today: `alrunner test`.

mod apps;
mod args;
mod coverage;
mod output;
mod run;
mod serve;
mod testdata;

use args::Command;
use std::process::ExitCode;

#[cfg(all(feature = "mimalloc", not(target_family = "wasm")))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // The tree-walking VM recurses on the native stack per AL call; give it room.
    let worker = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(real_main)
        .expect("spawn runner thread");
    match worker.join() {
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
