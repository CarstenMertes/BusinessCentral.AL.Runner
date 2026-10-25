//! Sibling operations (`AddAfterSelf`, `AddBeforeSelf`, `ReplaceWith`) on an element, attribute or document without a
//! parent (corpus xml/XmlElement.al, xml/lt-xml-attr-doc-http-members/P97AttrEdit.al, P97DocEdit.al, P97DocMisc.al).

use al_testkit::text;

fn run(body: &str) -> String {
    let src = format!(
        "codeunit 50100 C {{ procedure F(): Text var E: XmlElement; A: XmlAttribute; Doc: XmlDocument; N: XmlNode; I: Integer; S: Text; begin {body} end; }}"
    );
    text(al_testkit::run_with(&src, "C", "F", vec![]).map(|(v, _)| v))
}

const MSG: &str = "The requested operation cannot be performed in this context.";

#[test]
fn a_lone_element_raises_invalid_operation_for_an_element_argument() {
    // xml/XmlElement.al:AddAfterSelf_Root_Error `lone_after`, `lone_before`; ReplaceWith_Root_Unparented `lone`.
    let r = run("E := XmlElement.Create('lone');
         asserterror E.AddAfterSelf(XmlElement.Create('b')); S := GetLastErrorText() + '/' + GetLastErrorCode();
         asserterror E.AddBeforeSelf(XmlElement.Create('b')); S += '|' + GetLastErrorText();
         asserterror E.ReplaceWith(XmlElement.Create('b')); S += '|' + GetLastErrorText(); exit(S);");
    assert_eq!(r, format!("{MSG}/InvalidOperation|{MSG}|{MSG}"));
}

#[test]
fn a_detached_attribute_raises_invalid_operation_for_any_argument() {
    // P97AttrEdit.al:AddAfterSelf_Detached_Kinds `k14` (an unassigned XmlNode), *_Detached_Typed `t10` (an Integer).
    let r = run("A := XmlAttribute.Create('x', '2');
         asserterror A.AddAfterSelf(N); S := GetLastErrorText() + '/' + GetLastErrorCode();
         I := 42; asserterror A.AddBeforeSelf(I); S += '|' + GetLastErrorText();
         asserterror A.ReplaceWith('plain text'); S += '|' + GetLastErrorText(); exit(S);");
    assert_eq!(r, format!("{MSG}/InvalidOperation|{MSG}|{MSG}"));
}

#[test]
fn a_document_raises_invalid_operation_but_an_unassigned_node_argument_first() {
    // P97DocEdit.al:AddAfterSelf_RootOnly_Kinds `k1`, `k14`; AddBeforeSelf_RootOnly_Typed `t10`.
    let r = run("Doc := XmlDocument.Create(); Doc.Add(XmlElement.Create('r'));
         asserterror Doc.AddAfterSelf(XmlElement.Create('e')); S := GetLastErrorText() + '/' + GetLastErrorCode();
         I := 42; asserterror Doc.AddBeforeSelf(I); S += '|' + GetLastErrorText();
         asserterror Doc.ReplaceWith(N); S += '|' + GetLastErrorText() + '/' + GetLastErrorCode(); exit(S);");
    assert_eq!(
        r,
        format!(
            "{MSG}/InvalidOperation|{MSG}|Microsoft.Dynamics.Nav.Runtime.NavXmlNode variable not initialized./NotInitialized"
        )
    );
}

#[test]
fn an_element_whose_document_is_no_longer_referenced_is_refused() {
    // xml/XmlElement.al:530: BC keeps the document of a root alive after the local document variable is gone; the
    // runtime holds parents weakly, so the root would look parentless.
    let src = "codeunit 50100 C {
        local procedure Load(var Root: XmlElement) var Doc: XmlDocument; begin XmlDocument.ReadFrom('<a/>', Doc); Doc.GetRoot(Root); end;
        procedure F(): Text var E: XmlElement; begin Load(E); E.AddAfterSelf(XmlComment.Create('c')); exit('ok'); end; }";
    let r = al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v);
    assert!(
        matches!(&r, Err(al_builtins::native::Signal::Unsupported(m)) if m.contains("no longer referenced")),
        "{r:?}"
    );
}
