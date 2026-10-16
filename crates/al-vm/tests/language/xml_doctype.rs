//! `XmlDocumentType` (corpus xml/lt-xml-doctype-declaration/*; the corpus cases are the full oracle).

use al_testkit::text;

fn run(body: &str) -> String {
    let src = format!(
        "codeunit 50100 C {{ procedure F(): Text var Dt: XmlDocumentType; Doc: XmlDocument; S: Text; T: Text; begin {body} end; }}"
    );
    text(al_testkit::run_with(&src, "C", "F", vec![]).map(|(v, _)| v))
}

#[test]
fn a_document_type_is_made_read_written_and_added_to_a_document() {
    // DocTypeCreate.al:Create_FourArguments, DocTypeInDoc.al:DocumentCreate_WithDocType.
    assert_eq!(
        run("Dt := XmlDocumentType.Create('a', 'p', 's.dtd', '<!ELEMENT a EMPTY>'); Dt.GetName(S); Dt.GetSystemId(T); S += '|' + T;
             Doc := XmlDocument.Create(Dt); Doc.Add(XmlElement.Create('a')); Doc.WriteTo(T);
             exit(S + '|' + T + '|' + Format(Doc.GetChildNodes().Count()));"),
        "a|s.dtd|<?xml version=\"1.0\" encoding=\"utf-16\"?>\n<!DOCTYPE a PUBLIC \"p\" \"s.dtd\"[<!ELEMENT a EMPTY>]>\n<a />|2"
    );
}

#[test]
fn a_bad_name_raises_the_xml_exception_and_a_second_type_or_one_after_the_root_is_refused() {
    // DocTypeErrors.al:Create_InvalidNames; DocTypeInDoc.al:DocumentAdd_DocType, DocumentAdd_DocTypeAfterRoot.
    assert_eq!(
        run("asserterror Dt := XmlDocumentType.Create('a b', '', '', ''); S := GetLastErrorText(); Doc := XmlDocument.Create(); Doc.Add(XmlDocumentType.Create('a', '', '', ''));
             asserterror Doc.Add(XmlDocumentType.Create('b', '', '', '')); T := GetLastErrorText(); exit(S + '|' + T);"),
        "The following exception was encountered when processing XML data: 'The ' ' character, hexadecimal value 0x20, cannot be included in a name.' at line 0 and position 2.|The requested operation cannot be performed in this context."
    );
}

#[test]
fn a_type_made_with_a_name_only_has_members_that_are_not_initialized() {
    // DocTypeCreate.al:Create_NameOnly (af103), DocTypeErrors.al:Uninitialised_Getters.
    assert_eq!(
        run(
            "Dt := XmlDocumentType.Create('html'); asserterror Dt.GetName(S); S := GetLastErrorText(); Clear(Dt); asserterror Dt.GetName(T); exit(S + '|' + GetLastErrorText());"
        ),
        "Exception of type 'Microsoft.Dynamics.Nav.Types.Exceptions.NavNCLMemberNotInitializedException' was thrown.|Microsoft.Dynamics.Nav.Runtime.NavXmlDocumentType variable not initialized."
    );
}

#[test]
fn a_detached_text_cdata_comment_or_pi_raises_invalid_operation_for_sibling_operations_and_remove() {
    // xml/lt-xml-cdata-comment-pi-text/P90*Edit.al:AfterSelf_Detached_*, BeforeSelf_Detached_*, ReplaceWith_Detached_*,
    // Remove_Detached_Twice.
    let msg = "The requested operation cannot be performed in this context.";
    let r = run(
        "asserterror XmlCData.Create('x').AddAfterSelf(XmlComment.Create('c')); S := GetLastErrorText();
         asserterror XmlComment.Create('x').AddBeforeSelf(XmlComment.Create('c')); S += '|' + GetLastErrorText();
         asserterror XmlText.Create('x').ReplaceWith(XmlComment.Create('c')); S += '|' + GetLastErrorText();
         asserterror XmlCData.Create('x').Remove(); S += '|' + GetLastErrorText(); exit(S);",
    );
    assert_eq!(r, format!("{msg}|{msg}|{msg}|{msg}"));
}
