//! Typed instructions (docs/reviews/2026-10-bytecode-design.md, "Typed instructions"): the compiler picks them when it
//! knows both operand types, and they give what the generic instructions give, errors included.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_testkit::int;

fn code(body: &str) -> String {
    format!("codeunit 50100 C {{ {body} }}")
}

/// The instructions emitted for routine `F` of the source.
fn ops_of(src: &str) -> String {
    let l = al_testkit::compile(src);
    let (oi, ri) = l
        .program
        .objects
        .iter()
        .enumerate()
        .find_map(|(oi, o)| o.routines.iter().position(|r| r.name == "F").map(|ri| (oi, ri)))
        .expect("routine F");
    al_vm::exec_disassemble(&l.program, oi, ri)
        .expect("compiles")
        .join("\n")
}

fn call(src: &str, args: Vec<Value>) -> Result<Value, Signal> {
    al_testkit::run_with(src, "C", "F", args).map(|(v, _)| v)
}

#[test]
fn integer_arithmetic_compare_and_conditions_use_typed_instructions() {
    let ops = ops_of(&code(
        "procedure F(A: Integer; B: Integer): Integer var I: Integer; begin I := A * B; I := I + 1; I := I - 2; I := I div 3; I := I mod 4; \
         if I < A then I := 0; while I <= 10 do I += 1; repeat I -= 1; until I = 0; exit(I); end;",
    ));
    for want in [
        "MulInt",
        "AddIntK",
        "DivInt",
        "ModInt",
        "JumpUnlessLtInt",
        "JumpUnlessLeInt",
        "JumpUnlessEqInt",
    ] {
        assert!(ops.contains(want), "{want} missing in\n{ops}");
    }
    assert!(!ops.contains("BinI") && !ops.contains("JumpIfNot {"), "{ops}");
}

#[test]
fn boolean_logic_uses_typed_instructions() {
    let ops = ops_of(&code(
        "procedure F(A: Boolean; B: Boolean): Boolean var C: Boolean; begin C := A and B; C := C or not A; C := C xor B; if C then exit(true); exit(false); end;",
    ));
    for want in ["AndBool", "OrBool", "XorBool", "NotBool", "JumpIfNotBool"] {
        assert!(ops.contains(want), "{want} missing in\n{ops}");
    }
}

#[test]
fn operands_of_an_unknown_type_stay_generic() {
    // Variant operands and a Decimal compare are not typed in step 1.
    let ops = ops_of(&code(
        "procedure F(V: Variant; D: Decimal): Boolean begin if D < 1.5 then exit(true); exit(V = 3); end;",
    ));
    assert!(!ops.contains("Int"), "{ops}");
}

#[test]
fn typed_integer_operations_keep_the_overflow_and_division_errors() {
    let src = code(
        "procedure F(A: Integer; B: Integer; Op: Integer): Integer begin \
         case Op of 0: exit(A + B); 1: exit(A - B); 2: exit(A * B); 3: exit(A div B); 4: exit(A mod B); 5: exit(A + 1); 6: exit(A - 1); end; end;",
    );
    let edges = [0, 1, -1, 2, i32::MAX, i32::MIN];
    for &a in &edges {
        for &b in &edges {
            for op in 0..=6 {
                let got = call(&src, vec![Value::Integer(a), Value::Integer(b), Value::Integer(op)]);
                let want: Option<i32> = match op {
                    0 => a.checked_add(b),
                    1 => a.checked_sub(b),
                    2 => a.checked_mul(b),
                    3 if b != 0 => a.checked_div(b),
                    4 if b != 0 => Some(a.wrapping_rem(b)),
                    5 => a.checked_add(1),
                    6 => a.checked_sub(1),
                    _ => None,
                };
                match (got, want) {
                    (Ok(v), Some(w)) => assert_eq!(int(Ok(v)), w, "{a} op{op} {b}"),
                    (Err(Signal::Error(e)), None) => {
                        let zero = e.message.contains("divide by zero");
                        let over = e.message.contains("overflow");
                        assert!(zero || over, "{a} op{op} {b}: {}", e.message);
                    }
                    (got, want) => panic!("{a} op{op} {b}: got {got:?}, want {want:?}"),
                }
            }
        }
    }
}

#[test]
fn typed_compares_and_jumps_decide_like_the_operators() {
    let src = code(
        "procedure F(A: Integer; B: Integer; Op: Integer): Integer begin \
         case Op of 0: if A = B then exit(1); 1: if A <> B then exit(1); 2: if A < B then exit(1); 3: if A <= B then exit(1); \
         4: if A > B then exit(1); 5: if A >= B then exit(1); end; exit(0); end;",
    );
    for a in [-2, 0, 3] {
        for b in [-2, 0, 3] {
            for op in 0..=5 {
                let want = match op {
                    0 => a == b,
                    1 => a != b,
                    2 => a < b,
                    3 => a <= b,
                    4 => a > b,
                    _ => a >= b,
                };
                let got = int(call(
                    &src,
                    vec![Value::Integer(a), Value::Integer(b), Value::Integer(op)],
                ));
                assert_eq!(got, i32::from(want), "{a} op{op} {b}");
            }
        }
    }
}

#[test]
fn a_local_holding_another_kind_than_declared_still_gets_the_generic_result() {
    // List.Get fills an Integer variable with whatever the list holds; the typed instruction must not misread it.
    let src = code(
        "procedure F(): Integer var L: List of [BigInteger]; I: Integer; begin L.Add(2147483647); L.Get(1, I); exit(I mod 10); end;",
    );
    assert_eq!(int(call(&src, vec![])), 7);
}
