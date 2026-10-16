//! `Query` variable methods. The VM executes them (`al-vm/src/queries.rs`); the registry only routes
//! catalog overload ids to it. A method not listed here, such as `SaveAsCsv` to a file name, is refused at
//! lowering ("builtin ... is not implemented").

use crate::native::NativeFn;

macro_rules! query_methods {
    ($($id:literal => $method:literal),* $(,)?) => {
        pub const REGISTRY: &[(&str, NativeFn)] = &[
            $(($id, |c, r, a| c.query($method, r, a))),*
        ];
    };
}

query_methods! {
    "Query.Open#1" => "Open",
    "Query.Read#1" => "Read",
    "Query.Close#1" => "Close",
    "Query.SetRange#1" => "SetRange",
    "Query.SetFilter#1" => "SetFilter",
    "Query.GetFilter#1" => "GetFilter",
    "Query.GetFilters#1" => "GetFilters",
    "Query.ColumnName#1" => "ColumnName",
    "Query.ColumnCaption#1" => "ColumnCaption",
    "Query.ColumnNo#1" => "ColumnNo",
    "Query.TopNumberOfRows#1" => "TopNumberOfRows",
    "Query.SecurityFiltering#1" => "SecurityFiltering",
    "Query.SaveAsXml#2" => "SaveAsXml",
    "Query.SaveAsJson#1" => "SaveAsJson",
    "Query.SaveAsCsv#2" => "SaveAsCsv",
    "Query.SaveAsCsv#4" => "SaveAsCsvStatic",
}
