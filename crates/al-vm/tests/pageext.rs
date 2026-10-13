//! Page extensions in the test client end to end: AL source → compile → run (ADR-012 follow-up).
//! Each test names the corpus test it mirrors.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const OBJECTS: &str = r#"
table 52000 "PX Row"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[30]) { }
        field(3; Other; Text[30]) { }
        field(4; Extra; Text[30]) { }
        field(5; Trace; Text[250]) { }
        field(6; Flag; Boolean) { }
        field(7; Descr; Text[50]) { Caption = 'Severity'; }
    }
    keys { key(PK; "No.") { } }
}
table 52001 "PX Lock" { fields { field(1; "Code"; Code[20]) { } } keys { key(PK; "Code") { } } }

codeunit 52100 "PX Trace"
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(T: Text) begin Log += T; end;
    procedure Take(): Text var R: Text; begin R := Log; Log := ''; exit(R); end;
}

page 52001 "PX List"
{
    PageType = List;
    SourceTable = "PX Row";
    layout { area(Content) { repeater(Rows) {
        field("No."; Rec."No.") { }
        field(Name; Rec.Name) { }
    } } }
    actions { area(Processing) { } }
}

pageextension 52001 "PX List Ext" extends "PX List"
{
    layout
    {
        addafter(Name)
        {
            field(ExtExpression; Format(Rec."No.") + ':' + Rec.Name) { }
            field(ExtGlobalField; ExtGlobal)
            {
                trigger OnValidate() begin ExtGlobal := UpperCase(ExtGlobal) + '!'; end;
            }
        }
        addlast(Rows)
        {
            field(ExtRecField; Rec.Other) { Caption = 'Ext Other'; }
        }
    }
    actions
    {
        addlast(Processing)
        {
            action("Stamp Ext")
            {
                trigger OnAction() var T: Codeunit "PX Trace"; begin T.Add('stamp:' + Rec."No." + ';'); end;
            }
            action(Fails) { trigger OnAction() begin Error('ext action refused'); end; }
        }
    }
    trigger OnOpenPage() var T: Codeunit "PX Trace"; begin ExtGlobal := 'EXTOPENED'; T.Add('ext-open;'); end;
    trigger OnAfterGetRecord() var T: Codeunit "PX Trace"; begin T.Add('ext-agr:' + Rec."No." + ';'); end;
    var ExtGlobal: Text[20];
}

page 52002 "PX Card"
{
    PageType = Card;
    SourceTable = "PX Row";
    layout { area(Content) {
        field(NoCtl; Rec."No.") { }
        field(Name; Rec.Name)
        {
            trigger OnValidate() begin Rec.Trace := Rec.Trace + 'page;'; end;
            trigger OnAssistEdit() begin Rec.Trace := Rec.Trace + 'baseassist;'; end;
        }
        field(Other; Rec.Other) { trigger OnValidate() begin Rec.Trace := Rec.Trace + 'otherpage;'; end; }
        field(Extra; Rec.Extra) { }
        field(Trace; Rec.Trace) { Editable = false; }
        field(Plain; Rec.Descr) { }
        field(FlagCtl; Rec.Flag) { Editable = BaseEditable; }
        field(Gate; Rec.Flag) { Editable = BaseGate; }
    } }
    actions { area(Processing) {
        action(DoIt) { trigger OnAction() var T: Codeunit "PX Trace"; begin T.Add('doit;'); end; }
        action(GateAct) { Enabled = BaseGate; trigger OnAction() begin end; }
        action(ForExtRef) { trigger OnAction() var T: Codeunit "PX Trace"; begin T.Add('base-via-ext;'); end; }
    } area(Promoted) {
        actionref(DoIt_Promoted; DoIt) { }
        group(Category_Process) { actionref(DoIt_Grouped; DoIt) { } }
    } }
    var BaseEditable: Boolean; BaseGate: Boolean;
    trigger OnOpenPage() var T: Codeunit "PX Trace"; begin BaseEditable := true; T.Add('base-open;'); end;
}

pageextension 52002 "PX Card Ext" extends "PX Card"
{
    layout
    {
        modify(Name)
        {
            trigger OnBeforeValidate() begin
                Rec.Trace := Rec.Trace + 'before;';
                if Rec.Name = 'stop' then Error('stopped in OnBeforeValidate');
            end;
            trigger OnAfterValidate() begin Rec.Trace := Rec.Trace + 'after;'; end;
        }
        modify(Plain) { Caption = 'Modified Plain'; }
        modify(FlagCtl) { Editable = not Locked; }
        modify(Gate) { Editable = not Locked; }
        modify(Extra)
        {
            trigger OnDrillDown() begin Rec.Trace := Rec.Trace + 'extdrill;'; end;
            trigger OnAssistEdit() begin Rec.Trace := Rec.Trace + 'extassist;'; end;
            trigger OnLookup(var Text: Text): Boolean begin Rec.Trace := Rec.Trace + 'extlookup;'; exit(false); end;
        }
    }
    actions
    {
        modify(GateAct) { Enabled = not Locked; }
        addlast(Processing) { action(StampExt) { trigger OnAction() var T: Codeunit "PX Trace"; begin T.Add('ext:' + Rec."No." + ';'); end; } }
        addlast(Promoted)
        {
            actionref(StampExt_Promoted; StampExt) { }
            actionref(StampBase_Promoted; ForExtRef) { }
        }
    }
    var Locked: Boolean;
    trigger OnAfterGetCurrRecord()
    var L: Record "PX Lock";
    begin
        Locked := L.Get(Rec."No.");
    end;
}

pageextension 52003 "PX Card Other Ext" extends "PX Card"
{
    layout
    {
        modify(Other)
        {
            trigger OnBeforeValidate() begin Rec.Trace := Rec.Trace + 'otherbefore;'; end;
            trigger OnAfterValidate() begin Rec.Trace := Rec.Trace + 'otherafter;'; end;
        }
    }
}

page 52010 "PX Part"
{
    PageType = CardPart;
    layout { area(Content) { field(Msg; Message) { } } }
    var Message: Text;
    trigger OnOpenPage() var T: Codeunit "PX Trace"; begin T.Add('part-open;'); end;
}
page 52011 "PX Host"
{
    PageType = Card;
    SourceTable = "PX Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
}
pageextension 52011 "PX Host Ext" extends "PX Host"
{
    layout { addlast(Content) {
        part(ExtPart; "PX Part") { }
        part(ExtHiddenPart; "PX Part") { Visible = false; }
    } }
}

page 52020 "PX Twice"
{
    PageType = Card;
    SourceTable = "PX Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
}
pageextension 52021 "PX Twice A" extends "PX Twice" { trigger OnOpenPage() var T: Codeunit "PX Trace"; begin T.Add('twice-a;'); end; }
pageextension 52022 "PX Twice B" extends "PX Twice" { trigger OnOpenPage() var T: Codeunit "PX Trace"; begin T.Add('twice-b;'); end; }
page 52025 "PX Closing"
{
    PageType = Card;
    SourceTable = "PX Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
}
pageextension 52026 "PX Closing Ext" extends "PX Closing" { trigger OnQueryClosePage(CloseAction: Action): Boolean begin exit(true); end; }
page 52030 "PX Curr"
{
    PageType = Card;
    SourceTable = "PX Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
}
pageextension 52030 "PX Curr Ext" extends "PX Curr" { trigger OnOpenPage() begin CurrPage.Editable(false); end; }
page 52024 "PX Missing"
{
    PageType = Card;
    SourceTable = "PX Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
}
pageextension 52023 "PX Missing Target" extends "PX Missing" { layout { addlast(NoSuchGroup) { field(X; Rec.Name) { } } } }
"#;

fn compile(apps: Vec<Vec<(String, String)>>) -> (al_sema::workspace::Workspace, al_sema::lower::Lowered) {
    let n = apps.len();
    let apps = apps
        .into_iter()
        .enumerate()
        .map(|(i, files)| AppSource {
            id: format!("app{i}"),
            name: format!("app{i}"),
            // Each app depends on the one before it.
            dependencies: if i == 0 { vec![] } else { vec![format!("app{}", i - 1)] },
            files,
        })
        .collect::<Vec<_>>();
    assert_eq!(apps.len(), n);
    al_sema::lower::compile(apps, &Default::default())
}

fn run_apps(apps: Vec<Vec<(String, String)>>, routine: &str) -> Result<Value, Signal> {
    let (ws, l) = compile(apps);
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![(oi, ri)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        if let Some(why) = l.unsupported[o][r].first() {
            panic!(
                "{}.{} is blocked: {why}",
                l.program.objects[o].name, l.program.objects[o].routines[r].name
            );
        }
        stack.extend(l.calls[o][r].iter().copied());
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn run(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{OBJECTS}\ncodeunit 52200 C {{ {body} }}");
    run_apps(vec![vec![("f.al".into(), src)]], routine)
}

/// The reason the first routine of codeunit C that declares a TestPage of `page` is refused.
fn refusal(body: &str) -> String {
    let src = format!("{OBJECTS}\ncodeunit 52200 C {{ {body} }}");
    let (_, l) = compile(vec![vec![("f.al".into(), src)]]);
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    l.unsupported[oi]
        .iter()
        .find_map(|r| r.first().cloned())
        .expect("a refusal")
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

const SEED: &str = "R.\"No.\" := 'A'; R.Name := 'Alpha'; R.Other := 'o1'; R.Insert();
    R.\"No.\" := 'B'; R.Name := 'Bravo'; R.Other := 'o2'; R.Insert();";

// pageextensiontrigger/TestPageExtensionPageTriggers_Tests.al:
// PageextensionOnOpenPageRunsOnceWhenTheExtendedPageOpens, ...OnAfterGetRecordRunsForEachRowTheTestPageVisits,
// ...OnOpenPageIsRaisedBeforeTheFirstOnAfterGetRecord
#[test]
fn an_extension_trigger_runs_in_the_pages_context_for_each_row() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PX Row\"; L: TestPage \"PX List\"; T: Codeunit \"PX Trace\"; begin
                {SEED}
                L.OpenView(); L.First(); L.Next(); L.Close();
                exit(T.Take());
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "ext-open;ext-agr:A;ext-agr:B;");
}

// pageextensioncontrol/TestPageExtensionControlTrapped_Tests.al: TrappedPageRun_ExtExpressionControl_ReadsTheCurrentRow,
// _ExtGlobalControl_ReadsTheValueItsOnOpenPageAssigned, _ExtControlOnValidate_UpdatesTheGlobalTheControlShows;
// pageextensioncontrol/TestPageExtensionControlBinding_Tests.al (a control bound to Rec from the extension)
#[test]
fn extension_controls_are_bound_to_rec_to_an_expression_and_to_the_extensions_own_global() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PX Row\"; L: TestPage \"PX List\"; X: Text; begin
                {SEED}
                L.OpenView();
                L.First();
                X := L.ExtExpression.Value() + '|' + L.ExtRecField.Value() + '|' + L.ExtGlobalField.Value() + '|';
                L.Next();
                X += L.ExtExpression.Value() + '|' + L.ExtRecField.Value();
                L.Close();
                exit(X);
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "A:Alpha|o1|EXTOPENED|B:Bravo|o2");
}

#[test]
fn an_extension_controls_on_validate_runs_on_the_extensions_instance() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PX Row\"; L: TestPage \"PX List\"; X: Text; begin
                {SEED}
                L.OpenEdit();
                L.ExtGlobalField.SetValue('typed');
                X := L.ExtGlobalField.Value();
                L.Close();
                exit(X);
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "TYPED!");
}

// handlers/TestPageExtensionActionInvoke_Tests.al: ExtActionInvokeRunsAgainstThePagesCurrentRow,
// ExtActionInvokePropagatesAnErrorRaisedInsideOnAction; handlers/TestPageSpacedActionName_Tests.al: SpacedExtActionInvokeRunsItsTrigger
#[test]
fn an_extension_action_runs_its_trigger_against_the_current_row() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PX Row\"; L: TestPage \"PX List\"; T: Codeunit \"PX Trace\"; X: Text; begin
                {SEED}
                L.OpenEdit();
                L.First(); L.Next();
                L.\"Stamp Ext\".Invoke();
                X := T.Take();
                asserterror L.Fails.Invoke();
                exit(X + '|' + GetLastErrorText());
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "ext-open;ext-agr:A;ext-agr:B;stamp:B;|ext action refused");
}

// pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al:
// ModifiedControlBeforeAndAfterValidateWrapTheBaseControlsOwnOnValidate, TheOtherControlsOwnModifyBlockWrapsItsOwnBaseTrigger,
// AModifyBlockOnAnotherControlIsNotRaisedForThisOne
#[test]
fn modify_before_and_after_validate_wrap_the_base_controls_on_validate() {
    let r = run(
        "procedure F(): Text var C: TestPage \"PX Card\"; X: Text; begin
            C.OpenNew();
            C.NoCtl.SetValue('N1');
            C.Name.SetValue('value');
            X := C.Trace.Value() + '|';
            C.Other.SetValue('v');
            X += C.Trace.Value();
            C.Close();
            exit(X);
        end;",
        "F",
    );
    assert_eq!(
        text(r),
        "before;page;after;|before;page;after;otherbefore;otherpage;otherafter;"
    );
}

// TestModifiedControlValidateTriggers_Tests.al: AnErrorInOnBeforeValidateDiscardsThatTriggersOwnRecMutation,
// TheRaisingTriggerDidRunEvenThoughItsMutationIsDiscarded, AWriteAfterARefusedOneTracesFromTheRestoredBuffer
#[test]
fn an_error_in_on_before_validate_discards_its_mutation_and_the_next_write_traces_cleanly() {
    let r = run(
        "procedure F(): Text var C: TestPage \"PX Card\"; X: Text; begin
            C.OpenNew();
            C.NoCtl.SetValue('N2');
            asserterror C.Name.SetValue('stop');
            X := '[' + C.Trace.Value() + ']' + GetLastErrorText() + '|';
            C.Name.SetValue('value');
            X += C.Trace.Value();
            exit(X);
        end;",
        "F",
    );
    let t = text(r);
    assert!(t.starts_with("[]"), "{t}");
    assert!(t.contains("stopped in OnBeforeValidate"), "{t}");
    assert!(t.ends_with("|before;page;after;"), "{t}");
}

// TestModifiedControlValidateTriggers_Tests.al: AModifyBlocksOnDrillDownRunsForTheModifiedControl,
// AModifyBlocksOnLookupRunsForTheModifiedControl, ABaseControlsOwnOnAssistEditRuns, AModifyBlocksOnAssistEditRunsForTheModifiedControl,
// OnAssistEditIsRaisedOnlyForTheControlItWasCalledOn, OnAssistEditRunsOncePerCall
#[test]
fn drill_down_lookup_and_assist_edit_run_the_triggers_of_the_control_they_are_called_on() {
    let r = run(
        "procedure F(): Text var C: TestPage \"PX Card\"; X: Text; begin
            C.OpenNew();
            C.NoCtl.SetValue('N3');
            C.Extra.DrillDown();
            C.Extra.Lookup();
            C.Name.AssistEdit();
            C.Name.AssistEdit();
            C.Extra.AssistEdit();
            X := C.Trace.Value();
            C.Close();
            exit(X);
        end;",
        "F",
    );
    assert_eq!(text(r), "extdrill;extlookup;baseassist;baseassist;extassist;");
}

// handlers/TestPageExtensionModifyCaption_Tests.al: SamePage_ModifiedCaption_OnRecordControl_WinsOverFieldCaption;
// handlers/TestPageExtensionControlCaption_Tests.al: ExtRecordFieldControl_DeclaredCaption_WinsOverFieldCaption
#[test]
fn a_modified_or_declared_caption_wins_over_the_field_caption() {
    let r = run(
        "procedure F(): Text var C: TestPage \"PX Card\"; L: TestPage \"PX List\"; X: Text; begin
                C.OpenNew();
                X := C.Plain.Caption() + '|';
                C.Close();
                L.OpenView();
                X += L.ExtRecField.Caption();
                L.Close();
                exit(X);
            end;",
        "F",
    );
    assert_eq!(text(r), "Modified Plain|Ext Other");
}

// pageextensioncontrol/TestPageExtensionModifyEditable_Tests.al:
// SourcePageExtModifyEditable_SourcePage_FollowsTheExtensionsVariablePerRow, ..._ExtensionReplacesTheBase
#[test]
fn a_modified_editable_or_enabled_follows_the_extensions_variable_and_replaces_the_base() {
    let r = run(
        "procedure F(): Text var R: Record \"PX Row\"; K: Record \"PX Lock\"; C: TestPage \"PX Card\"; X: Text; begin
            R.\"No.\" := 'LOCKED'; R.Insert(); R.\"No.\" := 'OPEN'; R.Insert();
            K.Code := 'LOCKED'; K.Insert();
            C.OpenEdit();
            R.Get('LOCKED'); C.GoToRecord(R);
            X := Format(C.Plain.Editable()) + Format(C.FlagCtl.Editable()) + Format(C.GateAct.Enabled()) + '|';
            R.Get('OPEN'); C.GoToRecord(R);
            X += Format(C.Plain.Editable()) + Format(C.FlagCtl.Editable()) + Format(C.Gate.Editable()) + Format(C.GateAct.Enabled());
            C.Close();
            exit(X);
        end;",
        "F",
    );
    assert_eq!(text(r), "YesNoNo|YesYesYesYes");
}

// handlers/TestPagePromotedActionref_Tests.al: PromotedActionrefInvokeRunsItsTargetsTrigger, PromotedActionrefInsideAGroupRunsItsTargetsTrigger,
// PageExtensionPromotedActionrefRunsItsOwnExtensionActionsTrigger, PageExtensionPromotedActionrefRunsABasePageActionsTrigger
#[test]
fn an_actionref_invokes_its_targets_trigger_whether_base_or_extension_declares_either() {
    let r = run(
        "procedure F(): Text var C: TestPage \"PX Card\"; T: Codeunit \"PX Trace\"; begin
            C.OpenNew();
            C.DoIt_Promoted.Invoke();
            C.DoIt_Grouped.Invoke();
            C.StampExt_Promoted.Invoke();
            C.StampBase_Promoted.Invoke();
            C.Close();
            exit(T.Take());
        end;",
        "F",
    );
    assert_eq!(text(r), "base-open;doit;doit;ext:;base-via-ext;");
}

// pageextensionpart/TestPageExtensionPart.al: SourceHost_ExtensionPart_OpensWithTheHost, SourceHost_ExtensionPartVisibleFalse_IsNotOnThePage
#[test]
fn a_part_an_extension_adds_opens_with_its_host_and_hides_when_not_visible() {
    let r = run(
        "procedure F(): Text var H: TestPage \"PX Host\"; T: Codeunit \"PX Trace\"; X: Text; B: Boolean; begin
            H.OpenView();
            X := Format(H.ExtPart.Visible()) + '|' + T.Take() + '|';
            asserterror B := H.ExtHiddenPart.Visible();
            X += GetLastErrorText();
            H.Close();
            exit(X);
        end;",
        "F",
    );
    let t = text(r);
    assert!(t.starts_with("Yes|part-open;|"), "{t}");
    assert!(t.contains("was not found on the page"), "{t}");
}

// pageextensiontrigger/TestPageExtQualifiedExtends_Tests.al: ImportedAndQualifiedExtendsBothRunForAUniquelyNamedPage
// (two extensions declare OnOpenPage; both run once, the test states no order between them).
#[test]
fn two_extensions_declaring_the_same_page_trigger_both_run_in_application_order() {
    let r = run(
        "procedure F(): Text var P: TestPage \"PX Twice\"; T: Codeunit \"PX Trace\"; begin P.OpenView(); P.Close(); exit(T.Take()); end;",
        "F",
    );
    assert_eq!(text(r), "twice-a;twice-b;");
}

// A trigger that answers a Boolean cannot be combined with the page's own answer: not pinned, so refused.
#[test]
fn an_extension_trigger_that_answers_a_boolean_is_refused() {
    let why = refusal("procedure F() var P: TestPage \"PX Closing\"; begin P.OpenView(); end;");
    assert!(
        why.contains("OnQueryClosePage") && why.contains("not supported"),
        "{why}"
    );
}

#[test]
fn an_extension_target_that_the_page_does_not_have_is_refused() {
    let why = refusal("procedure F() var P: TestPage \"PX Missing\"; begin P.OpenView(); end;");
    assert!(why.contains("NoSuchGroup"), "{why}");
}

const TWO_APPS_BASE: &str = r#"
table 52300 "PX Dep Row" { fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } } keys { key(PK; "No.") { } } }
page 52300 "PX Dep Page"
{
    PageType = Card;
    SourceTable = "PX Dep Row";
    layout { area(Content) {
        field(Both; Rec.Name) { Caption = 'Host Both'; }
        field(Twice; Rec.Name) { Caption = 'Host Twice'; }
        field(DepOnly; Rec.Name) { Caption = 'Host Dep'; }
    } }
}
pageextension 52301 "PX Dep Ext" extends "PX Dep Page"
{
    layout { modify(Both) { Caption = 'Dependency Both'; } modify(DepOnly) { Caption = 'Dependency Own'; } }
}
"#;

const TWO_APPS_MAIN: &str = r#"
// A lower object id than the dependency's extension, and still the one that wins.
pageextension 52300 "PX Main Low Ext" extends "PX Dep Page"
{
    layout { modify(Both) { Caption = 'Main Both'; } }
}
// Two extensions of this app modify one control: the higher object id wins, whatever the order written.
pageextension 52399 "PX Main High" extends "PX Dep Page"
{
    layout { modify(Twice) { Caption = 'High Id Twice'; } }
}
pageextension 52302 "PX Main Low" extends "PX Dep Page"
{
    layout { modify(Twice) { Caption = 'Low Id Twice'; } }
}
codeunit 52301 C
{
    procedure F(): Text
    var P: TestPage "PX Dep Page";
    begin
        P.OpenNew();
        exit(P.Both.Caption() + '|' + P.Twice.Caption() + '|' + P.DepOnly.Caption());
    end;
}
"#;

// handlers/TestPageExtensionModifyCaption_Tests.al: DependencyPage_ModifiedByBothApps_ThisAppWins_EvenWithTheLowerId (FxBoth2Ctl),
// DependencyPage_ModifiedCaption_FromItsOwnApp, SamePage_TwoExtensionsOfOneApp_HigherIdWins_WhateverTheOrder (Twice2Ctl)
#[test]
fn the_app_that_depends_on_the_other_wins_a_modify_and_within_an_app_the_higher_id_does() {
    let r = run_apps(
        vec![
            vec![("dep.al".into(), TWO_APPS_BASE.into())],
            vec![("main.al".into(), TWO_APPS_MAIN.into())],
        ],
        "F",
    );
    assert_eq!(text(r), "Main Both|High Id Twice|Dependency Own");
}

// pageextensioncontrol/TestPageExtensionControlTrapped_Tests.al: TrappedPageRun_UndeclaredControlId_IsNotFound;
// autoformat/TestAutoFormat.al:135 AutoFormat_GetFieldByTableFieldNo_Errors
#[test]
fn get_field_of_an_id_the_page_does_not_declare_is_not_found() {
    let r = run(
        "procedure F(): Text var L: TestPage \"PX List\"; X: Text; begin
            L.OpenView();
            asserterror X := L.GetField(12345).Value();
            exit(GetLastErrorText());
        end;",
        "F",
    );
    assert_eq!(text(r), "The field with ID = 12345 is not found on the page.");
}

// handlers/TestPageExtensionActionInvoke_Tests.al (the extension's code runs in the page's context) with
// handlers/TestPageControlEditableRoutes.al (CurrPage.Editable(false) in OnOpenPage): an extension reaches the page through CurrPage.
#[test]
fn an_extensions_code_reaches_the_page_through_curr_page() {
    let r = run(
        "procedure F(): Text var R: Record \"PX Row\"; H: TestPage \"PX Curr\"; X: Text; begin
            R.\"No.\" := 'A'; R.Insert();
            H.OpenEdit();
            X := Format(H.NoCtl.Editable());
            H.Close();
            exit(X);
        end;",
        "F",
    );
    assert_eq!(text(r), "No");
}
