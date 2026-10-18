"""The AL probe files the catalog generator opens in the AL language server.

Each probe is a small AL file with one cursor position (the character U+00A7, removed before the
file is sent). The generator asks the server for completions (or signature help) at the cursor and
records what the server answers. The probe texts only choose where to ask; every name in the
catalog comes from the server's answers.
"""

CURSOR = "§"

# Objects the probes refer to (a record, a page with a field, an action and a part, a report with
# a data item and a request page, a query, an xmlport, an enum, an interface, a codeunit).
OBJECTS = """table 50100 "Probe Table"
{
    fields
    {
        field(1; "Probe Field"; Integer) { }
        field(2; "Probe Text"; Text[30]) { }
        field(3; "Probe Blob"; Blob) { }
        field(4; "Probe Media"; Media) { }
        field(5; "Probe MediaSet"; MediaSet) { }
    }
    keys { key(PK; "Probe Field") { } }
}

page 50100 "Probe Page"
{
    PageType = Card;
    SourceTable = "Probe Table";
    layout
    {
        area(Content)
        {
            field("Probe Field"; Rec."Probe Field") { }
            part(ProbePart; "Probe Part") { }
        }
    }
    actions
    {
        area(Processing)
        {
            action(ProbeAction) { trigger OnAction() begin end; }
        }
    }
}

page 50101 "Probe Part"
{
    PageType = ListPart;
    SourceTable = "Probe Table";
    layout { area(Content) { repeater(Rows) { field("Probe Text"; Rec."Probe Text") { } } } }
}

report 50100 "Probe Report"
{
    ProcessingOnly = true;
    dataset { dataitem(ProbeItem; "Probe Table") { } }
    requestpage
    {
        layout { area(Content) { field(ProbeOption; ProbeOption) { } } }
    }
    var
        ProbeOption: Boolean;
}

query 50100 "Probe Query"
{
    elements { dataitem(ProbeItem; "Probe Table") { column(ProbeColumn; "Probe Field") { } } }
}

xmlport 50100 "Probe XmlPort"
{
    schema { textelement(Root) { tableelement(ProbeItem; "Probe Table") { fieldelement(F; ProbeItem."Probe Field") { } } } }
}

enum 50100 "Probe Enum"
{
    value(0; Zero) { }
}

interface "Probe Interface"
{
    procedure Probe();
}

codeunit 50101 "Probe Codeunit"
{
    procedure Probe() begin end;
}

permissionset 50100 "Probe Set"
{
    Assignable = true;
}
"""

# Declarations whose members the probes list (`X.`): owner -> AL type. The owner is the type name
# the runtime catalog uses. Types not listed here are declared as their plain name.
DECLARATIONS = {
    "Record": 'Record "Probe Table"',
    "Codeunit": 'Codeunit "Probe Codeunit"',
    "Page": 'Page "Probe Page"',
    "Report": 'Report "Probe Report"',
    "Query": 'Query "Probe Query"',
    "Xmlport": 'XmlPort "Probe XmlPort"',
    "Enum": 'Enum "Probe Enum"',
    "TestPage": 'TestPage "Probe Page"',
    "TestRequestPage": 'TestRequestPage "Probe Report"',
    "List": "List of [Integer]",
    "Dictionary": "Dictionary of [Integer, Text]",
    "Interface": 'Interface "Probe Interface"',
    "Label": "Label 'Probe'",
    "TextConst": "TextConst ENU = 'Probe'",
    "Code": "Code[20]",
    "Text": "Text",
}

# Data type names (from the server's own type completion) that are spelled differently as an owner.
OWNER_OF_TYPE = {"XmlPort": "Xmlport"}

# Types the probes do not declare: they need a definition the probe cannot give (an array size,
# option members, a .NET assembly, a control add-in).
NOT_DECLARED = {"array", "Option", "DotNet", "ControlAddIn"}

# Names tried as static receivers (`Name.`). The server answers which of them have members; a name
# without members is dropped.
STATIC_CANDIDATES = [
    "Session", "Database", "System", "Dialog", "CompanyProperty", "NavApp", "ProductName",
    "TaskScheduler", "IsolatedStorage", "NumberSequence", "Debugger", "SessionInformation", "File",
    "Report", "Page", "Codeunit", "Xmlport", "Query", "Text", "Guid", "ErrorInfo", "Version",
    "RequestPage", "HttpClient", "JsonObject", "XmlDocument", "XmlElement", "XmlAttribute",
    "XmlNode", "XmlText", "XmlComment", "XmlCData", "XmlDeclaration", "XmlDocumentType",
    "XmlProcessingInstruction", "XmlNamespaceManager", "XmlNameTable", "Media", "MediaSet",
    "Notification", "DataTransfer", "TextBuilder", "BigText", "Variant", "Blob", "RecordRef",
    "FieldRef", "KeyRef", "Date", "Time", "DateTime", "Integer", "Decimal", "Boolean",
    "BigInteger", "Byte", "Char", "Duration", "ModuleInfo", "SecretText", "FileUpload",
]

# Members the probe objects declare themselves (not part of the language).
PROBE_MEMBERS = {"Probe"}

# Members reached through another member: owner -> (declaration owner, expression prefix).
SUB_MEMBERS = {
    "TestField": ("TestPage", 'X."Probe Field"'),
    "TestAction": ("TestPage", "X.ProbeAction"),
    "TestPart": ("TestPage", "X.ProbePart"),
    "TestFilter": ("TestPage", "X.Filter"),
    "Blob": ("Record", 'X."Probe Blob"'),
    "Media": ("Record", 'X."Probe Media"'),
    "MediaSet": ("Record", 'X."Probe MediaSet"'),
}

# Instance members only reachable inside an object (`CurrPage.`, `CurrReport.`, `currXMLport.`,
# `RequestOptionsPage.`): owner -> AL text where `{expr}` is the member expression.
INSIDE = {
    "Page": 'page 50111 "Probe Inside"\n{\n    SourceTable = "Probe Table";\n    trigger OnOpenPage()\n    begin\n        CurrPage.{expr}\n    end;\n}\n',
    "Report": 'report 50111 "Probe Inside"\n{\n    trigger OnPreReport()\n    begin\n        CurrReport.{expr}\n    end;\n}\n',
    "Xmlport": 'xmlport 50111 "Probe Inside"\n{\n    schema { textelement(Root) { } }\n    trigger OnPreXmlPort()\n    begin\n        currXMLport.{expr}\n    end;\n}\n',
    "RequestPage": 'report 50111 "Probe Inside"\n{\n    requestpage\n    {\n        trigger OnOpenPage()\n        begin\n            RequestOptionsPage.{expr}\n        end;\n    }\n}\n',
}

# Static members of an enum type.
ENUM_STATIC = 'Enum::"Probe Enum"'


def member_probe(decl, expr):
    return (
        "codeunit 50110 \"Probe Members\"\n{\n    procedure P()\n    var\n"
        f"        X: {decl};\n    begin\n        {expr}\n    end;\n}}\n"
    )


def statement_probe():
    return (
        "codeunit 50110 \"Probe Members\"\n{\n    procedure P()\n    var\n        X: Integer;\n"
        f"    begin\n        {CURSOR}\n    end;\n}}\n"
    )


def type_probe():
    return (
        "codeunit 50110 \"Probe Members\"\n{\n    procedure P()\n    var\n"
        f"        X: {CURSOR}\n    begin\n    end;\n}}\n"
    )


def attribute_probe():
    return f"codeunit 50110 \"Probe Members\"\n{{\n    [{CURSOR}]\n    procedure P()\n    begin\n    end;\n}}\n"


def event_probe(object_type, object_ref):
    return (
        "codeunit 50110 \"Probe Members\"\n{\n"
        f"    [EventSubscriber(ObjectType::{object_type}, {object_ref}, '{CURSOR}', '', false, false)]\n"
        "    local procedure P()\n    begin\n    end;\n}\n"
    )


# Contexts for triggers and properties: owner -> AL text with the cursor where a member of that
# owner goes. `@` is replaced with `trigger <cursor>` (triggers) or `<cursor>` (properties).
T = "@"
CONTEXTS = {
    "Codeunit": f"codeunit 50120 \"Probe Ctx\"\n{{\n    {T}\n}}\n",
    "Table": f"table 50120 \"Probe Ctx\"\n{{\n    {T}\n    fields {{ field(1; A; Integer) {{ }} }}\n}}\n",
    "Field": f"table 50120 \"Probe Ctx\"\n{{\n    fields\n    {{\n        field(1; A; Integer)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "Key": f"table 50120 \"Probe Ctx\"\n{{\n    fields {{ field(1; A; Integer) {{ }} }}\n    keys\n    {{\n        key(PK; A)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "FieldGroup": f"table 50120 \"Probe Ctx\"\n{{\n    fields {{ field(1; A; Integer) {{ }} }}\n    fieldgroups\n    {{\n        fieldgroup(DropDown; A)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "TableExtension": f"tableextension 50120 \"Probe Ctx\" extends \"Probe Table\"\n{{\n    {T}\n}}\n",
    "FieldModify": f"tableextension 50120 \"Probe Ctx\" extends \"Probe Table\"\n{{\n    fields\n    {{\n        modify(\"Probe Text\")\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "Page": f"page 50120 \"Probe Ctx\"\n{{\n    SourceTable = \"Probe Table\";\n    {T}\n}}\n",
    "PageField": f"page 50120 \"Probe Ctx\"\n{{\n    SourceTable = \"Probe Table\";\n    layout\n    {{\n        area(Content)\n        {{\n            field(F; Rec.\"Probe Field\")\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageGroup": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            group(G)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageRepeater": f"page 50120 \"Probe Ctx\"\n{{\n    PageType = List;\n    SourceTable = \"Probe Table\";\n    layout\n    {{\n        area(Content)\n        {{\n            repeater(G)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageCueGroup": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            cuegroup(G)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageGrid": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            grid(G)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageFixed": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            fixed(G)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageArea": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "PagePart": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            part(P; \"Probe Part\")\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageSystemPart": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(FactBoxes)\n        {{\n            systempart(P; Notes)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageChartPart": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            chartpart(P; 'Probe')\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageLabel": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            label(L)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageUserControl": f"page 50120 \"Probe Ctx\"\n{{\n    layout\n    {{\n        area(Content)\n        {{\n            usercontrol(U; WebPageViewer)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageActionArea": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "PageAction": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            action(A)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageActionGroup": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            group(G)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageActionSeparator": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            separator(S)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageActionRef": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            action(A) {{ }}\n        }}\n        area(Promoted)\n        {{\n            actionref(R; A)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageCustomAction": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            customaction(C)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageSystemAction": f"page 50120 \"Probe Ctx\"\n{{\n    PageType = PromptDialog;\n    actions\n    {{\n        area(SystemActions)\n        {{\n            systemaction(Generate)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageFileUploadAction": f"page 50120 \"Probe Ctx\"\n{{\n    actions\n    {{\n        area(Processing)\n        {{\n            fileuploadaction(F)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "PageView": f"page 50120 \"Probe Ctx\"\n{{\n    PageType = List;\n    SourceTable = \"Probe Table\";\n    views\n    {{\n        view(V)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "PageExtension": f"pageextension 50120 \"Probe Ctx\" extends \"Probe Page\"\n{{\n    {T}\n}}\n",
    "PageControlModify": f"pageextension 50120 \"Probe Ctx\" extends \"Probe Page\"\n{{\n    layout\n    {{\n        modify(\"Probe Field\")\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "PageActionModify": f"pageextension 50120 \"Probe Ctx\" extends \"Probe Page\"\n{{\n    actions\n    {{\n        modify(ProbeAction)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "PageCustomizationObject": f"pagecustomization \"Probe Ctx\" customizes \"Probe Page\"\n{{\n    {T}\n}}\n",
    "AllPageCustomization": f"pagecustomization \"Probe Ctx\" customizes \"Probe Page\"\n{{\n    layout\n    {{\n        modify(\"Probe Field\")\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "Report": f"report 50120 \"Probe Ctx\"\n{{\n    {T}\n}}\n",
    "ReportDataItem": f"report 50120 \"Probe Ctx\"\n{{\n    dataset\n    {{\n        dataitem(D; \"Probe Table\")\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "ReportColumn": f"report 50120 \"Probe Ctx\"\n{{\n    dataset\n    {{\n        dataitem(D; \"Probe Table\")\n        {{\n            column(C; \"Probe Field\")\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "ReportLayout": f"report 50120 \"Probe Ctx\"\n{{\n    rendering\n    {{\n        layout(L)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "RequestPage": f"report 50120 \"Probe Ctx\"\n{{\n    requestpage\n    {{\n        {T}\n    }}\n}}\n",
    "ReportExtension": f"reportextension 50120 \"Probe Ctx\" extends \"Probe Report\"\n{{\n    {T}\n}}\n",
    "ReportExtensionModify": f"reportextension 50120 \"Probe Ctx\" extends \"Probe Report\"\n{{\n    dataset\n    {{\n        modify(ProbeItem)\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "RequestPageExtension": f"reportextension 50120 \"Probe Ctx\" extends \"Probe Report\"\n{{\n    requestpage\n    {{\n        {T}\n    }}\n}}\n",
    "Query": f"query 50120 \"Probe Ctx\"\n{{\n    {T}\n    elements {{ dataitem(D; \"Probe Table\") {{ }} }}\n}}\n",
    "QueryDataItem": f"query 50120 \"Probe Ctx\"\n{{\n    elements\n    {{\n        dataitem(D; \"Probe Table\")\n        {{\n            {T}\n        }}\n    }}\n}}\n",
    "QueryColumn": f"query 50120 \"Probe Ctx\"\n{{\n    elements\n    {{\n        dataitem(D; \"Probe Table\")\n        {{\n            column(C; \"Probe Field\")\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "QueryFilter": f"query 50120 \"Probe Ctx\"\n{{\n    elements\n    {{\n        dataitem(D; \"Probe Table\")\n        {{\n            filter(F; \"Probe Field\")\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "XmlPort": f"xmlport 50120 \"Probe Ctx\"\n{{\n    {T}\n    schema {{ textelement(Root) {{ }} }}\n}}\n",
    "XmlPortTableElement": f"xmlport 50120 \"Probe Ctx\"\n{{\n    schema\n    {{\n        textelement(Root)\n        {{\n            tableelement(T; \"Probe Table\")\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "XmlPortFieldElement": f"xmlport 50120 \"Probe Ctx\"\n{{\n    schema\n    {{\n        textelement(Root)\n        {{\n            tableelement(T; \"Probe Table\")\n            {{\n                fieldelement(F; T.\"Probe Field\")\n                {{\n                    {T}\n                }}\n            }}\n        }}\n    }}\n}}\n",
    "XmlPortTextElement": f"xmlport 50120 \"Probe Ctx\"\n{{\n    schema\n    {{\n        textelement(Root)\n        {{\n            textelement(E)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "XmlPortFieldAttribute": f"xmlport 50120 \"Probe Ctx\"\n{{\n    schema\n    {{\n        textelement(Root)\n        {{\n            tableelement(T; \"Probe Table\")\n            {{\n                fieldattribute(F; T.\"Probe Field\")\n                {{\n                    {T}\n                }}\n            }}\n        }}\n    }}\n}}\n",
    "XmlPortTextAttribute": f"xmlport 50120 \"Probe Ctx\"\n{{\n    schema\n    {{\n        textelement(Root)\n        {{\n            textattribute(A)\n            {{\n                {T}\n            }}\n        }}\n    }}\n}}\n",
    "EnumType": f"enum 50120 \"Probe Ctx\"\n{{\n    {T}\n    value(0; Zero) {{ }}\n}}\n",
    "EnumValue": f"enum 50120 \"Probe Ctx\"\n{{\n    value(0; Zero)\n    {{\n        {T}\n    }}\n}}\n",
    "EnumExtension": f"enumextension 50120 \"Probe Ctx\" extends \"Probe Enum\"\n{{\n    {T}\n}}\n",
    "Interface": f"interface \"Probe Ctx\"\n{{\n    {T}\n}}\n",
    "PermissionSet": f"permissionset 50120 \"Probe Ctx\"\n{{\n    {T}\n}}\n",
    "PermissionSetExtension": f"permissionsetextension 50120 \"Probe Ctx\" extends \"Probe Set\"\n{{\n    {T}\n}}\n",
    "Entitlement": f"entitlement \"Probe Ctx\"\n{{\n    {T}\n}}\n",
    "Profile": f"profile \"Probe Ctx\"\n{{\n    {T}\n}}\n",
    "ControlAddInObject": f"controladdin \"Probe Ctx\"\n{{\n    {T}\n}}\n",
}

# More contexts for the triggers of one owner: a codeunit's triggers depend on its Subtype.
TRIGGER_EXTRA_CONTEXTS = {
    "Codeunit": [
        f"codeunit 50120 \"Probe Ctx\"\n{{\n    Subtype = {subtype};\n    {T}\n}}\n"
        for subtype in ("Install", "Upgrade", "Test", "TestRunner")
    ],
}

# Trigger owners whose triggers sit in a context above: trigger owner -> context owner. The
# trigger owner names are the ones the coverage map's group tables use.
TRIGGER_CONTEXTS = {
    "Codeunit": "Codeunit",
    "Table": "Table",
    "Field": "Field",
    "TableExtension": "TableExtension",
    "FieldExtension": "FieldModify",
    "Page": "Page",
    "Control": "PageField",
    "Action": "PageAction",
    "FileUploadAction": "PageFileUploadAction",
    "PageExtension": "PageExtension",
    "ControlExtension": "PageControlModify",
    "ActionExtension": "PageActionModify",
    "Report": "Report",
    "ReportDataItem": "ReportDataItem",
    "RequestPage": "RequestPage",
    "ReportExtension": "ReportExtension",
    "ReportExtensionDataSetModify": "ReportExtensionModify",
    "RequestPageExtension": "RequestPageExtension",
    "XmlPort": "XmlPort",
    "XmlPortTableElement": "XmlPortTableElement",
    "XmlPortFieldElement": "XmlPortFieldElement",
    "XmlPortTextElement": "XmlPortTextElement",
    "XmlPortFieldAttribute": "XmlPortFieldAttribute",
    "XmlPortTextAttribute": "XmlPortTextAttribute",
    "Query": "Query",
}
