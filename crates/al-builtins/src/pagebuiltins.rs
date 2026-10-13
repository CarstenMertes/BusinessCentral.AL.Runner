//! `TestPage`, `TestPart`, `TestField`, `TestAction` and `TestFilter` methods
//! (ADR-012). Every one is implemented by the VM's test client; the registry
//! only routes catalog overload ids to it. A method not listed here is refused
//! at lowering ("builtin ... is not implemented").

use crate::native::NativeFn;

macro_rules! page_methods {
    ($($id:literal => $method:literal),* $(,)?) => {
        pub const REGISTRY: &[(&str, NativeFn)] = &[
            $(($id, |c, r, a| c.page($method, r, a))),*
        ];
    };
}

page_methods! {
    "TestPage.OpenEdit#1" => "TestPage.OpenEdit",
    "TestPage.OpenView#1" => "TestPage.OpenView",
    "TestPage.OpenNew#1" => "TestPage.OpenNew",
    "TestPage.Close#1" => "TestPage.Close",
    "TestPage.First#1" => "TestPage.First",
    "TestPage.Last#1" => "TestPage.Last",
    "TestPage.Next#1" => "TestPage.Next",
    "TestPage.Previous#1" => "TestPage.Previous",
    "TestPage.GoToKey#1" => "TestPage.GoToKey",
    "TestPage.GoToRecord#1" => "TestPage.GoToRecord",
    "TestField.Value#1" => "TestField.Value",
    "TestField.AsInteger#1" => "TestField.AsInteger",
    "TestField.AsDecimal#1" => "TestField.AsDecimal",
    "TestField.AsBoolean#1" => "TestField.AsBoolean",
    "TestField.AsDate#1" => "TestField.AsDate",
    "TestField.AsTime#1" => "TestField.AsTime",
    "TestField.AsDateTime#1" => "TestField.AsDateTime",
    "TestField.AssertEquals#1" => "TestField.AssertEquals",
    "TestField.Caption#1" => "TestField.Caption",
    "TestField.Editable#1" => "TestField.Editable",
    "TestField.Enabled#1" => "TestField.Enabled",
    "TestField.Visible#1" => "TestField.Visible",
    "TestField.SetValue#1" => "TestField.SetValue",
    "TestField.Activate#1" => "TestField.Activate",
    "TestField.ValidationErrorCount#1" => "TestField.ValidationErrorCount",
    "TestField.GetValidationError#1" => "TestField.GetValidationError",
    "TestPage.ValidationErrorCount#1" => "TestPage.ValidationErrorCount",
    "TestPage.GetValidationError#1" => "TestPage.GetValidationError",
    "TestPage.New#1" => "TestPage.New",
    "Page.Update#1" => "Page.Update",
    "Page.SaveRecord#1" => "Page.SaveRecord",
    "TestPage.OK#1" => "TestPage.OK",
    "TestPage.Cancel#1" => "TestPage.Cancel",
    "TestPage.Yes#1" => "TestPage.Yes",
    "TestPage.No#1" => "TestPage.No",
    "TestPage.Edit#1" => "TestPage.Edit",
    "TestPage.View#1" => "TestPage.View",
    "TestPage.Caption#1" => "TestPage.Caption",
    "TestPage.Editable#1" => "TestPage.Editable",
    "TestPage.Trap#1" => "TestPage.Trap",
    "Page.Run#1" => "Page.Run",
    "Page.Run#4" => "Page.RunStatic",
    "Page.RunModal#1" => "Page.RunModal",
    "Page.RunModal#5" => "Page.RunModalStatic",
    "Page.LookupMode#1" => "Page.LookupMode",
    "Page.Close#1" => "Page.Close",
    "TestAction.Invoke#1" => "TestAction.Invoke",
    "TestAction.Enabled#1" => "TestAction.Enabled",
    "TestAction.Visible#1" => "TestAction.Visible",
}
