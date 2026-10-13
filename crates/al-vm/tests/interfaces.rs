//! Interface variables, dispatch and enum implementations, end to end.
//!
//! Each test cites the corpus test that pins the behaviour.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

/// corpus _fixtures/interfaces/IALTCompute.al
const COMPUTE: &str = r#"
interface IALTCompute
{
    procedure Compute(X: Integer): Integer;
}

codeunit 60011 ALTDouble implements IALTCompute
{
    procedure Compute(X: Integer): Integer
    begin
        exit(2 * X);
    end;
}

codeunit 60012 ALTSquare implements IALTCompute
{
    procedure Compute(X: Integer): Integer
    begin
        exit(X * X);
    end;
}
"#;

/// corpus types/TestEnumDefaultImplGreeter.Interface.al, ...Impls.Codeunit.al, ...Kind.Enum.al
const EDI: &str = r#"
interface "EDI Greeter"
{
    procedure Greet(): Text;
}

codeunit 60310 "EDI Default Impl" implements "EDI Greeter"
{
    procedure Greet(): Text begin exit('DEFAULT'); end;
}

codeunit 60311 "EDI Own Impl" implements "EDI Greeter"
{
    procedure Greet(): Text begin exit('OWN'); end;
}

codeunit 60312 "EDI Unknown Impl" implements "EDI Greeter"
{
    procedure Greet(): Text begin exit('UNKNOWN'); end;
}

enum 60309 "EDI Kind" implements "EDI Greeter"
{
    Extensible = true;
    DefaultImplementation = "EDI Greeter" = "EDI Default Impl";
    UnknownValueImplementation = "EDI Greeter" = "EDI Unknown Impl";

    value(0; "Falls Back") { Caption = 'Falls Back'; }
    value(1; "Has Own") { Caption = 'Has Own'; Implementation = "EDI Greeter" = "EDI Own Impl"; }
}
"#;

/// corpus types/TestEnumExtImpl*.al
const EEI: &str = r#"
interface "EEI Marker"
{
    procedure Marker(): Text;
}

codeunit 60890 "EEI Base Impl" implements "EEI Marker"
{
    procedure Marker(): Text begin exit('BASE'); end;
}

codeunit 60891 "EEI Ext Impl" implements "EEI Marker"
{
    procedure Marker(): Text begin exit('EXTENSION'); end;
}

codeunit 60892 "EEI Default Impl" implements "EEI Marker"
{
    procedure Marker(): Text begin exit('DEFAULT'); end;
}

enum 60893 "EEI Kind" implements "EEI Marker"
{
    Extensible = true;
    DefaultImplementation = "EEI Marker" = "EEI Default Impl";

    value(0; Base) { Implementation = "EEI Marker" = "EEI Base Impl"; }
}

enumextension 60890 "EEI Kind Ext" extends "EEI Kind"
{
    value(10; Extended) { Implementation = "EEI Marker" = "EEI Ext Impl"; }
    value(20; "Extended No Impl") { }
}
"#;

fn compile(src: &str) -> (al_sema::workspace::Workspace, al_sema::lower::Lowered) {
    al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
        }],
        &Default::default(),
    )
}

fn run(fixtures: &str, body: &str, routine: &str) -> Result<Value, Signal> {
    run_with(fixtures, body, routine, &[])
}

/// Like `run`, passing enum values `(enum name, ordinal)` as the routine's arguments. The
/// corpus builds them with `Enum.FromInteger`, which is a separate builtin.
fn run_with(fixtures: &str, body: &str, routine: &str, enums: &[(&str, i32)]) -> Result<Value, Signal> {
    let src = format!("{fixtures}\ncodeunit 50100 T {{ {body} }}");
    let (ws, l) = compile(&src);
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let oi = l.program.objects.iter().position(|o| o.name == "T").expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .expect("routine");
    assert!(blocked[oi][ri].is_none(), "refused: {:?}", blocked[oi][ri]);
    let args = enums
        .iter()
        .map(|(n, ord)| {
            let e = l.program.objects.iter().position(|o| o.name == *n).expect("enum");
            Value::Enum(e as u32, *ord)
        })
        .collect();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, args).map(|(v, _)| v)
}

/// The refusal reason of routine `routine` of codeunit T, if any.
fn refusal(fixtures: &str, body: &str, routine: &str) -> Option<String> {
    let src = format!("{fixtures}\ncodeunit 50100 T {{ {body} }}");
    let (ws, l) = compile(&src);
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let oi = l.program.objects.iter().position(|o| o.name == "T").expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .expect("routine");
    l.unsupported[oi][ri].first().cloned()
}

fn int(r: Result<Value, Signal>) -> i32 {
    match r {
        Ok(Value::Integer(i)) => i,
        other => panic!("{other:?}"),
    }
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

// corpus codeunit/TestCodeunitInterface.al:14-26 (Interface_Assign_DoubleImpl_ComputesDouble)
#[test]
fn a_codeunit_variable_assigns_to_an_interface_and_dispatches() {
    let r = run(
        COMPUTE,
        "procedure P(): Integer var C: Interface IALTCompute; D: Codeunit ALTDouble; begin C := D; exit(C.Compute(10)); end;",
        "P",
    );
    assert_eq!(int(r), 20);
}

// corpus codeunit/TestCodeunitInterface.al:40-60 (Interface_Reassign_ChangesImpl)
#[test]
fn reassigning_an_interface_switches_the_implementation() {
    let r = run(
        COMPUTE,
        "procedure P(): Integer var C: Interface IALTCompute; D: Codeunit ALTDouble; S: Codeunit ALTSquare; A: Integer; begin C := D; A := C.Compute(4); C := S; exit(A * 100 + C.Compute(4)); end;",
        "P",
    );
    assert_eq!(int(r), 8 * 100 + 16);
}

// corpus codeunit/TestCodeunitInterface.al:85-96 (negative input)
#[test]
fn dispatch_passes_arguments_through() {
    let r = run(
        COMPUTE,
        "procedure P(): Integer var C: Interface IALTCompute; D: Codeunit ALTDouble; begin C := D; exit(C.Compute(-3)); end;",
        "P",
    );
    assert_eq!(int(r), -6);
}

// corpus types/TestEnumDefaultImplTests.Codeunit.al:30-42
#[test]
fn an_enum_value_without_its_own_implementation_uses_the_default() {
    let r = run(
        EDI,
        "procedure P(): Text var G: Interface \"EDI Greeter\"; begin G := Enum::\"EDI Kind\"::\"Falls Back\"; exit(G.Greet()); end;",
        "P",
    );
    assert_eq!(text(r), "DEFAULT");
}

// corpus types/TestEnumDefaultImplTests.Codeunit.al:44-56
#[test]
fn an_enum_value_with_its_own_implementation_prefers_it() {
    let r = run(
        EDI,
        "procedure P(): Text var G: Interface \"EDI Greeter\"; begin G := Enum::\"EDI Kind\"::\"Has Own\"; exit(G.Greet()); end;",
        "P",
    );
    assert_eq!(text(r), "OWN");
}

// corpus types/TestEnumDefaultImplTests.Codeunit.al:58-78 (the enum value built with FromInteger)
#[test]
fn an_undeclared_ordinal_uses_the_unknown_value_implementation() {
    let r = run_with(
        EDI,
        "procedure P(K: Enum \"EDI Kind\"): Text var G: Interface \"EDI Greeter\"; begin G := K; exit(G.Greet()); end;",
        "P",
        &[("EDI Kind", 99)],
    );
    assert_eq!(text(r), "UNKNOWN");
}

// corpus types/TestEnumDefaultImplTests.Codeunit.al:80-100
#[test]
fn declared_ordinals_through_from_integer_keep_the_declared_chain() {
    let r = run_with(
        EDI,
        "procedure P(K: Enum \"EDI Kind\"): Text var G: Interface \"EDI Greeter\"; begin G := K; exit(G.Greet()); end;",
        "P",
        &[("EDI Kind", 0)],
    );
    assert_eq!(text(r), "DEFAULT");
    let r = run_with(
        EDI,
        "procedure P(K: Enum \"EDI Kind\"): Text var G: Interface \"EDI Greeter\"; begin G := K; exit(G.Greet()); end;",
        "P",
        &[("EDI Kind", 1)],
    );
    assert_eq!(text(r), "OWN");
}

// corpus types/TestEnumExtImplTests.Codeunit.al:33-76
#[test]
fn enum_extension_values_carry_their_own_and_the_base_default_implementation() {
    let r = run(
        EEI,
        "procedure P(): Text var M: Interface \"EEI Marker\"; R: Text; begin \
         M := Enum::\"EEI Kind\"::Extended; R := M.Marker(); \
         M := Enum::\"EEI Kind\"::\"Extended No Impl\"; R += '/' + M.Marker(); \
         M := Enum::\"EEI Kind\"::Base; R += '/' + M.Marker(); exit(R); end;",
        "P",
    );
    assert_eq!(text(r), "EXTENSION/DEFAULT/BASE");
}

// corpus types/TestInterfaceCodeunitStateField.al:11-27: the implementation keeps its own
// global state across the cast.
#[test]
fn the_implementation_keeps_its_instance_state() {
    let fixtures = r#"
interface IState { procedure Bump(): Integer; }
codeunit 60371 StateImpl implements IState
{
    var N: Integer;
    procedure Bump(): Integer begin N += 1; exit(N); end;
}
enum 60370 StateKind implements IState { value(0; V) { Implementation = IState = StateImpl; } }
"#;
    let r = run(
        fixtures,
        "procedure P(): Integer var S: Interface IState; K: Enum StateKind; begin K := K::V; S := K; S.Bump(); exit(S.Bump()); end;",
        "P",
    );
    assert_eq!(int(r), 2);
}

#[test]
fn an_unassigned_interface_is_refused_at_run_time() {
    let r = run(
        COMPUTE,
        "procedure P(): Integer var C: Interface IALTCompute; begin exit(C.Compute(1)); end;",
        "P",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

#[test]
fn interfaces_inside_collections_are_refused() {
    let why = refusal(
        COMPUTE,
        "procedure P() var L: List of [Interface IALTCompute]; begin end;",
        "P",
    );
    assert!(why.is_some_and(|w| w.contains("interface")), "refused");
}

#[test]
fn interface_return_values_are_refused() {
    let why = refusal(
        COMPUTE,
        "procedure P(): Interface IALTCompute var D: Codeunit ALTDouble; begin exit(D); end;",
        "P",
    );
    assert!(why.is_some_and(|w| w.contains("interface")), "refused");
}

#[test]
fn var_interface_parameters_are_refused() {
    let why = refusal(COMPUTE, "procedure P(var C: Interface IALTCompute) begin end;", "P");
    assert!(why.is_some_and(|w| w.contains("interface")), "refused");
}

#[test]
fn extended_interfaces_are_refused() {
    let fixtures = "interface IBase { procedure A(); } interface IMore extends IBase { procedure B(); }";
    let why = refusal(fixtures, "procedure P() var M: Interface IMore; begin end;", "P");
    assert!(why.is_some_and(|w| w.contains("extends")), "refused");
}
