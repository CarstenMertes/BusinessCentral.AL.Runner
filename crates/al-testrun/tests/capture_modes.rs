//! The captured form of the bytecode (ADR-030 on ADR-034) records what the tree walker records: the same values, loops,
//! iterations, statements and messages, for programs that use every loop kind, `break`, `continue`, `exit`, `asserterror`,
//! calls with `var` arguments and `case`.
//!
//! The mode (`AL_EXEC`) is read once per process, so the test runs this binary twice, once per mode, and compares.

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_vm::CaptureConfig;

const PROGRAMS: &[&str] = &[
    // Every loop kind, break and continue, nested loops.
    "codeunit 50100 X {
        trigger OnRun() var I: Integer; J: Integer; S: Integer; T: Text; L: List of [Integer]; V: Integer;
        begin
            while I < 5 do begin I += 1; if I = 2 then continue; if I = 4 then break; S += I; end;
            repeat J += 1; S += J; until J >= 3;
            for I := 1 to 3 do for J := 3 downto 2 do S += I * J;
            L.Add(4); L.Add(5); L.Add(6);
            foreach V in L do begin if V = 5 then continue; T += Format(V); end;
            for I := 1 to 4 do begin if I = 3 then break; S += 1; end;
            Message('done %1', S);
        end;
    }",
    // exit inside a loop of a callee, var arguments, case, a recursive call.
    "codeunit 50101 X {
        procedure Find(Limit: Integer): Integer var K: Integer; begin for K := 1 to 100 do begin if K * K > Limit then exit(K); end; exit(0); end;
        procedure Bump(var N: Integer; By: Integer) begin N += By; end;
        procedure Fact(N: Integer): Integer begin if N <= 1 then exit(1); exit(N * Fact(N - 1)); end;
        trigger OnRun() var A: Integer; B: Integer; C: Text;
        begin
            A := Find(50);
            Bump(A, 3); Bump(A, 4);
            case A of 10..12: C := 'low'; 13: C := 'mid'; else C := 'high'; end;
            B := Fact(5);
            while A < 20 do begin Bump(A, 5); case A mod 2 of 0: B += 1; 1: B += 2; end; end;
        end;
    }",
    // asserterror inside loops, an error that leaves a loop and a statement.
    "codeunit 50102 X {
        procedure Boom(N: Integer) begin if N = 2 then Error('boom %1', N); end;
        trigger OnRun() var I: Integer; Hits: Integer; J: Integer;
        begin
            for I := 1 to 3 do begin
                asserterror Boom(I);
                if GetLastErrorText() <> '' then Hits += 1;
                J := 0;
                while J < 2 do begin J += 1; asserterror begin if J = 1 then Error('x'); end; end;
            end;
            asserterror begin for I := 1 to 5 do begin Hits += 1; if I = 2 then Error('stop'); end; end;
            Hits += 10;
        end;
    }",
    // A loop whose body breaks out of nested ifs, and continue in repeat, and a loop in a callee called from a loop.
    "codeunit 50103 X {
        procedure Sum(N: Integer): Integer var I: Integer; R: Integer; begin for I := 1 to N do R += I; exit(R); end;
        trigger OnRun() var I: Integer; T: Integer; K: Integer;
        begin
            for I := 1 to 4 do begin
                if I > 1 then begin if I = 3 then begin if T >= 0 then break; end; end;
                T += Sum(I);
            end;
            repeat K += 1; if K = 2 then continue; T += K; until K >= 4;
        end;
    }",
];

fn run(code: &str) -> String {
    let apps = vec![AppSource {
        id: "a".into(),
        name: "a".into(),
        dependencies: vec![],
        files: vec![("a.al".into(), code.into())],
    }];
    let rt = CorpusRuntime::try_run_suites_with(
        apps,
        vec![al_sema::AppKind::Workspace],
        &[],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun {
            name: String::new(),
            app: Some(0),
            runner: None,
        }],
        &RunOptions {
            execute: true,
            capture: CaptureConfig {
                values: true,
                loops: true,
            },
            ..Default::default()
        },
    )
    .unwrap();
    let r = &rt.results()[0];
    format!("{:?}\n{:#?}", r.outcome, r.captured.as_ref().map(|c| &c.captured))
}

/// The child: prints what the capture collected, between markers.
#[test]
#[ignore]
fn dump_captures() {
    println!("<<<");
    for p in PROGRAMS {
        println!("{}", run(p));
    }
    println!(">>>");
}

fn dump(mode: &str) -> String {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "dump_captures", "--ignored", "--nocapture", "--test-threads=1"])
        .env("AL_EXEC", mode)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{mode}: {text}\n{}", String::from_utf8_lossy(&out.stderr));
    let (a, b) = (text.find("<<<").unwrap(), text.find(">>>").unwrap());
    text[a..b].to_string()
}

#[test]
fn the_bytecode_captures_what_the_tree_walker_captures() {
    let (tree, bytecode) = (dump("tree"), dump("bytecode"));
    assert!(tree.contains("iterations"), "the programs capture loops:\n{tree}");
    assert!(tree.len() > 2000, "{tree}");
    assert_eq!(tree, bytecode);
}
