//! XmlPort import and export (Format = Xml): `SetSource`, `SetDestination`, `SetTableView`,
//! `Import` and `Export`, instance and static forms.
//!
//! Every rule is pinned by a corpus test, cited where it is applied. What no test pins is
//! refused (`Signal::Unsupported`) and listed in docs/probes-needed.md: the exact bytes of an
//! export (declaration, whitespace), the texts of import errors, field types other than
//! Integer, Decimal, Text and Code, tables with triggers or event subscribers, and so on.

use crate::records::Rec;
use crate::transactions::TxModel;
use crate::{AlError, Session, Signal};
use al_builtins::builtin_id::ids;
use al_builtins::describe::Describe;
use al_builtins::ops;
use al_builtins::record_op::RecordOp;
use al_builtins::value::{Instance, LibraryValue, RtType, Value};
use al_builtins::xmlport::{Elem, Node, ReadFailure};
use al_data::{Store, position};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type Inst = Rc<RefCell<Instance>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn xerr(message: impl Into<String>, code: &str) -> Signal {
    Signal::Error(Box::new(AlError {
        message: message.into(),
        code: code.into(),
        call_stack: String::new(),
        ..Default::default()
    }))
}

/// xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_SchemaMismatch_ThrowsXmlPortInvalidXml
fn schema_error(why: impl AsRef<str>) -> Signal {
    xerr(
        format!("Imported XML cannot validate with the schema: {}", why.as_ref()),
        "XmlPortInvalidXml",
    )
}

/// xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_BadInteger_ThrowsEvaluate
fn evaluate_error(text: &str, type_name: &str) -> Signal {
    xerr(
        format!("The value \"{text}\" can't be evaluated into type {type_name}."),
        "Evaluate",
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Import,
    Export,
}

/// The schema of the xmlport instance `inst` is, kept alive independently of the session.
struct Schema {
    engine: Arc<crate::Engine>,
    object: usize,
}

impl Schema {
    fn xp(&self) -> &ir::XmlPort {
        self.engine.program.objects[self.object]
            .xmlport
            .as_ref()
            .expect("checked by Session::xp_schema")
    }
    fn name(&self) -> &str {
        &self.engine.program.objects[self.object].name
    }
    fn node(&self, i: usize) -> &ir::XmlNode {
        &self.xp().nodes[i]
    }
    fn children(&self, i: usize) -> Vec<usize> {
        (0..self.xp().nodes.len())
            .filter(|&c| self.xp().nodes[c].parent == Some(i))
            .collect()
    }
}

impl Session {
    pub(crate) fn xmlport_builtin(
        &mut self,
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
        ret_used: bool,
    ) -> Result<Value, Signal> {
        match id {
            ids::XMLPORT_SETSOURCE_1 | ids::XMLPORT_SETDESTINATION_1 => {
                let inst = self.xp_instance(receiver)?;
                let schema = self.xp_schema(&inst)?;
                let (slot, want) = if id == ids::XMLPORT_SETSOURCE_1 {
                    (schema.xp().source_slot, "InStream")
                } else {
                    (schema.xp().destination_slot, "OutStream")
                };
                match args.first() {
                    Some(v @ Value::Library(n, _)) if n.eq_ignore_ascii_case(want) => {
                        inst.borrow_mut().globals[slot] = v.clone();
                        Ok(Value::Null)
                    }
                    other => unsupported(format!("{id} with {}", other.describe())),
                }
            }
            // The separators of a text xmlport: a value a program set reads back as it was given; a declared one reads as the
            // characters it stands for (`<CR/LF>` is `\r\n`, `<None>` is empty); an undeclared one reads as the default
            // (platform/lt-xmlport-session-navapp-cookie/P94XpDelim.al:Getters_*, Setters_*, *_Variants).
            ids::XMLPORT_FIELDDELIMITER_1
            | ids::XMLPORT_FIELDSEPARATOR_1
            | ids::XMLPORT_RECORDSEPARATOR_1
            | ids::XMLPORT_TABLESEPARATOR_1 => {
                let inst = self.xp_instance(receiver)?;
                let kind: u8 = match id {
                    ids::XMLPORT_FIELDDELIMITER_1 => 0,
                    ids::XMLPORT_FIELDSEPARATOR_1 => 1,
                    ids::XMLPORT_RECORDSEPARATOR_1 => 2,
                    _ => 3,
                };
                match args.first() {
                    Some(v) => {
                        let Some(text) = al_builtins::native::text_of(v) else {
                            return unsupported(format!("{id} with {}", v.describe()));
                        };
                        let mut i = inst.borrow_mut();
                        i.xp_text.retain(|(k, _)| *k != kind);
                        i.xp_text.push((kind, text.to_string()));
                        Ok(Value::Null)
                    }
                    None => {
                        if let Some((_, v)) = inst.borrow().xp_text.iter().find(|(k, _)| *k == kind) {
                            return Ok(Value::Text(v.clone().into()));
                        }
                        let object = inst.borrow().object as usize;
                        let Some(xp) = self.engine.program.objects[object].xmlport.as_ref() else {
                            return unsupported(format!("object {object} is not an xmlport"));
                        };
                        let (declared, default) = match kind {
                            0 => (&xp.field_delimiter, "\""),
                            1 => (&xp.field_separator, ","),
                            2 => (&xp.record_separator, "\\r\\n"),
                            _ => (&xp.table_separator, "\\r\\n\\r\\n"),
                        };
                        Ok(Value::Text(
                            match declared {
                                Some(d) => declared_separator(d)?,
                                None => default.to_string(),
                            }
                            .into(),
                        ))
                    }
                }
            }
            // Break, BreakUnbound and Quit end the trigger that calls them (platform/lt-xmlport-session-navapp-cookie/P94XpPathFlow.al:
            // Export_Break, Export_BreakUnbound, Export_Quit); what the run does next is `xp_flow`. Called from a procedure the
            // trigger called, the Exit leaves only that procedure: pinned only where the trigger ends with the call.
            ids::XMLPORT_BREAK_1 | ids::XMLPORT_BREAKUNBOUND_1 | ids::XMLPORT_QUIT_1 => {
                let inst = self.xp_instance(receiver)?;
                let trigger_depth = inst
                    .borrow()
                    .xp_text
                    .iter()
                    .find(|(k, _)| *k == 9)
                    .and_then(|(_, v)| v.parse::<usize>().ok());
                match trigger_depth {
                    Some(d) if self.depth == d + 1 || self.depth == d + 2 => {}
                    _ => return unsupported(format!("{id} outside the triggers where it is pinned")),
                }
                let kind = match id {
                    ids::XMLPORT_BREAK_1 => "1",
                    ids::XMLPORT_BREAKUNBOUND_1 => "2",
                    _ => "3",
                };
                let mut s = inst.borrow_mut();
                s.xp_text.retain(|(k, _)| *k != 8);
                s.xp_text.push((8, kind.to_string()));
                Err(Signal::Exit(None))
            }
            // CurrentPath(): see `xp_run_trigger`.
            ids::XMLPORT_CURRENTPATH_1 => {
                let inst = self.xp_instance(receiver)?;
                let path = inst
                    .borrow()
                    .xp_text
                    .iter()
                    .find(|(k, _)| *k == 7)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                Ok(Value::Text(path.into()))
            }
            // Filename(): empty until set, per instance (P94XpFilename.al: Fresh_Empty, Set_ReadBack, Set_TwoInstancesIndependent).
            ids::XMLPORT_FILENAME_1 => {
                let inst = self.xp_instance(receiver)?;
                match args.first() {
                    Some(v) => {
                        let Some(text) = al_builtins::native::text_of(v) else {
                            return unsupported(format!("{id} with {}", v.describe()));
                        };
                        let text = text.to_string();
                        let mut s = inst.borrow_mut();
                        s.xp_text.retain(|(k, _)| *k != 5);
                        s.xp_text.push((5, text));
                        Ok(Value::Null)
                    }
                    None => {
                        let s = inst.borrow();
                        let name = s
                            .xp_text
                            .iter()
                            .find(|(k, _)| *k == 5)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default();
                        Ok(Value::Text(name.into()))
                    }
                }
            }
            // TextEncoding(): MSDos until declared or set (UTF8 when declared so); the setter takes the member (P94XpEncoding.al:
            // Getter_Defaults, Setter_ReadBack). Which bytes an export then writes is separate.
            ids::XMLPORT_TEXTENCODING_1 => {
                const MEMBERS: [&str; 4] = ["MSDos", "UTF8", "UTF16", "Windows"];
                let inst = self.xp_instance(receiver)?;
                let members = || -> std::sync::Arc<[String]> {
                    MEMBERS.iter().map(|m| (*m).to_string()).collect::<Vec<_>>().into()
                };
                match args.first() {
                    Some(Value::Option(i, _)) if (0..4).contains(i) => {
                        let mut s = inst.borrow_mut();
                        s.xp_text.retain(|(k, _)| *k != 4);
                        s.xp_text.push((4, i.to_string()));
                        Ok(Value::Null)
                    }
                    Some(other) => unsupported(format!("{id} with {}", other.describe())),
                    None => {
                        if let Some((_, v)) = inst.borrow().xp_text.iter().find(|(k, _)| *k == 4) {
                            return Ok(Value::Option(v.parse().unwrap_or(0), members()));
                        }
                        let object = inst.borrow().object as usize;
                        match self.engine.program.objects[object].xmlport.as_ref() {
                            Some(xp) if xp.utf8 => Ok(Value::Option(1, members())),
                            Some(xp) if xp.text_encoding_declared => {
                                unsupported("TextEncoding() of an xmlport that declares another encoding (not pinned)")
                            }
                            Some(_) => Ok(Value::Option(0, members())),
                            None => unsupported(format!("object {object} is not an xmlport")),
                        }
                    }
                }
            }
            ids::XMLPORT_SETTABLEVIEW_1 => {
                let inst = self.xp_instance(receiver)?;
                self.xp_set_view(&inst, args.first())?;
                Ok(Value::Null)
            }
            ids::XMLPORT_EXPORT_1 | ids::XMLPORT_IMPORT_1 => {
                let inst = self.xp_instance(receiver)?;
                let dir = if matches!(id, ids::XMLPORT_EXPORT_1 | ids::XMLPORT_EXPORT_2) {
                    Dir::Export
                } else {
                    Dir::Import
                };
                self.xp_run(&inst, dir, ret_used, false)
            }
            // The static forms run a fresh instance of the xmlport with that id.
            ids::XMLPORT_EXPORT_2 | ids::XMLPORT_IMPORT_2 => {
                let dir = if matches!(id, ids::XMLPORT_EXPORT_1 | ids::XMLPORT_EXPORT_2) {
                    Dir::Export
                } else {
                    Dir::Import
                };
                let Some(Value::Integer(n)) = args.first() else {
                    return unsupported(format!("{id} with an id that is not an integer"));
                };
                let Some(object) = self
                    .engine
                    .program
                    .objects
                    .iter()
                    .position(|o| o.is_kind(al_ir::ObjKind::XmlPort) && o.id == u32::try_from(*n).ok())
                else {
                    return unsupported(format!(
                        "{id} of id {n}, which no xmlport declares (error text not pinned)"
                    ));
                };
                let inst = self.instance(object);
                let schema = self.xp_schema(&inst)?;
                let (slot, want) = match dir {
                    Dir::Export => (schema.xp().destination_slot, "OutStream"),
                    Dir::Import => (schema.xp().source_slot, "InStream"),
                };
                match args.get(1) {
                    Some(v @ Value::Library(n, _)) if n.eq_ignore_ascii_case(want) => {
                        inst.borrow_mut().globals[slot] = v.clone();
                    }
                    other => return unsupported(format!("{id} with the stream {}", other.describe())),
                }
                // The record only seeds the table view
                // (xmlport/TestXmlPortObject.al:XmlPort_Import_StaticWithRecord_GivenVariableUnchangedWithoutExplicitGet,
                // :XmlPort_Export_StaticWithRecord_RespectsFilters).
                if let Some(rec) = args.get(2) {
                    self.xp_set_view(&inst, Some(rec))?;
                }
                self.xp_run(&inst, dir, ret_used, true)
            }
            other => unsupported(format!("builtin {other}")),
        }
    }

    /// The xmlport instance behind a receiver, with its globals made.
    fn xp_instance(&mut self, receiver: Option<Value>) -> Result<Inst, Signal> {
        match receiver {
            Some(Value::Codeunit(rc)) => {
                self.ensure_initialized(&rc);
                Ok(rc)
            }
            other => unsupported(format!("an XmlPort method on {}", other.describe())),
        }
    }

    /// The schema of `inst`'s object; refuses an xmlport the runtime cannot run.
    fn xp_schema(&self, inst: &Inst) -> Result<Schema, Signal> {
        let object = inst.borrow().object as usize;
        let schema = Schema {
            engine: self.engine.clone(),
            object,
        };
        match &schema.engine.program.objects[object].xmlport {
            Some(xp) => match xp.unsupported.first() {
                None => Ok(schema),
                Some(why) => unsupported(format!("XmlPort \"{}\": {why}", schema.name())),
            },
            None => unsupported(format!("object {object} is not an xmlport")),
        }
    }

    fn xp_record(&self, inst: &Inst, node: &ir::XmlNode) -> Result<Rec, Signal> {
        match node.slot.map(|s| inst.borrow().globals[s].clone()) {
            Some(Value::Record(rc)) => Ok(rc),
            other => unsupported(format!(
                "table element {} holds {other}",
                node.name,
                other = other.describe()
            )),
        }
    }

    /// `SetTableView`: the filters of the given record go to the first table element
    /// (xmlport/TestXmlPortObject.al:XmlPort_SetTableView_RestrictsExportedRows).
    fn xp_set_view(&mut self, inst: &Inst, view: Option<&Value>) -> Result<(), Signal> {
        let schema = self.xp_schema(inst)?;
        let Some(first) = schema.xp().nodes.iter().find(|n| n.kind == ir::XmlNodeKind::Table) else {
            return unsupported(format!(
                "SetTableView on XmlPort \"{}\", which has no table element",
                schema.name()
            ));
        };
        let Some(Value::Record(view)) = view else {
            return unsupported(format!("SetTableView with {}", view.describe()));
        };
        let target = self.xp_record(inst, first)?;
        let v = view.borrow();
        if v.table != target.borrow().table {
            return unsupported("SetTableView with a record of another table (error text not pinned)");
        }
        if v.temporary || v.marked_only {
            return unsupported("SetTableView with a temporary or marked record (not pinned)");
        }
        // The view's key and direction order the export, ties included
        // (xmlport/TestXmlPortTriggersReplaceViewContracts.al:Export_OnBeforePassField_Runs_AndSetTableViewFiltersAndOrders,
        // corpus: .
        let mut t = target.borrow_mut();
        t.filters = v.filters.clone();
        t.filter_group = v.filter_group;
        t.key = v.key;
        t.adhoc = v.adhoc.clone();
        t.ascending = v.ascending;
        t.field_descending = v.field_descending;
        Ok(())
    }

    fn xp_run(&mut self, inst: &Inst, dir: Dir, ret_used: bool, static_form: bool) -> Result<Value, Signal> {
        let schema = self.xp_schema(inst)?;
        // xmlport/TestXmlPortExportImportContracts.al:XmlPort_WrongDirection_Throws
        match (dir, schema.xp().direction) {
            (Dir::Import, ir::XmlDirection::Export) => {
                return Err(xerr(
                    format!("The XMLPort '{}' is designed for export only.", schema.name()),
                    "XmlPortExportDirection",
                ));
            }
            (Dir::Export, ir::XmlDirection::Import) => {
                return Err(xerr(
                    format!("The XMLPort '{}' is designed for import only.", schema.name()),
                    "XmlPortImportDirection",
                ));
            }
            _ => {}
        }
        // Constructs no corpus test pins are refused when the port runs, not when it is declared
        // (`ir::XmlPort::run_unsupported`).
        if let Some(why) = schema.xp().run_unsupported.first() {
            return unsupported(format!("XmlPort \"{}\": {why}", schema.name()));
        }
        // OnPreXmlPort runs when the port starts, in both directions
        // (handlers/TestPageActionRunObjectKinds_Tests.al:ProbeControlEachTargetRecordsItsOwnExecution pins
        // it for an export); the other object triggers are not pinned and refused.
        let routines = &schema.engine.program.objects[schema.object].routines;
        let mut pre = None;
        let mut post = None;
        for (i, r) in routines.iter().enumerate().filter(|(_, r)| r.is_trigger) {
            if r.name.eq_ignore_ascii_case("OnPreXmlPort") {
                pre = Some(i);
            } else if r.name.eq_ignore_ascii_case("OnPostXmlPort") {
                post = Some(i);
            } else if r.name.eq_ignore_ascii_case("OnInitXmlPort") {
                return unsupported(format!(
                    "XmlPort \"{}\": trigger {} is not supported yet",
                    schema.name(),
                    r.name
                ));
            }
        }
        if let Some(r) = pre {
            self.call(schema.object, r, inst.clone(), Vec::new())?;
        }
        Self::xp_clear_flow(inst);
        let run = match dir {
            Dir::Export => self.xp_export(inst, &schema, ret_used),
            Dir::Import => self.xp_import(inst, &schema, ret_used, static_form),
        };
        // Quit ends the run at once: no OnPostXmlPort, and the run succeeded (Export_Quit).
        if matches!(run, Err(Signal::Exit(None))) && Self::xp_flow(inst) == Some(Flow::Quit) {
            Self::xp_clear_flow(inst);
            return Ok(if ret_used { Value::Boolean(true) } else { Value::Null });
        }
        let result = run?;
        // A Break, BreakUnbound or Quit anywhere else is not pinned.
        if Self::xp_flow(inst).is_some() {
            Self::xp_clear_flow(inst);
            return unsupported(
                "Break, BreakUnbound or Quit in a trigger other than OnAfterGetRecord of an export (not pinned)",
            );
        }
        // OnPostXmlPort runs after the data was passed, in both directions (platform/lt-xmlport-session-navapp-cookie/
        // P94XpPathFlow.al and P94XpFilename.al log it last); after an error it does not run. After a guarded run that
        // returned false it is not pinned.
        if let Some(r) = post {
            if matches!(result, Value::Boolean(false)) {
                return unsupported("OnPostXmlPort after a guarded run that returned false (not pinned)");
            }
            self.call(schema.object, r, inst.clone(), Vec::new())?;
        }
        Ok(result)
    }

    // ---------------------------------------------------------------- import

    fn xp_import(&mut self, inst: &Inst, schema: &Schema, guarded: bool, static_form: bool) -> Result<Value, Signal> {
        if guarded {
            // A guarded import needs a transaction world of its own, which BC refuses to open
            // while the caller holds an uncommitted write; the static form turns the refusal
            // into false, the instance form raises it
            // (xmlport/TestXmlPortImportWriteTx.al:StaticGuardedImport_WithUncommittedWrite_ReturnsFalse_AndImportsNothing,
            // :InstanceGuardedImport_WithUncommittedWrite_IsRefused_AndImportsNothing).
            if self.in_write_transaction() {
                if self.tx_model == TxModel::AutoRollback {
                    return unsupported("guarded XmlPort.Import with a pending write under AutoRollback (not pinned)");
                }
                return if static_form {
                    // Whether the trapped refusal becomes the last error, and with what text, is not
                    // pinned: reading it is refused.
                    self.last_error = al_builtins::native::unpinned_error();
                    Ok(Value::Boolean(false))
                } else {
                    Err(crate::transactions::transaction_stopped())
                };
            }
            if self.tx.try_depth > 0 {
                return unsupported("guarded XmlPort.Import inside a TryFunction (not pinned)");
            }
        }
        // Import begins a transaction of its own, so a write is allowed inside it even under
        // TransactionModel::None, and the transaction ends with the import
        // (codeunit/TestWriteTxTestBoundary.al:WriteTx_Test12_AnXmlPortImportMayWriteUnderNone).
        let began_world = self.no_transaction();
        self.tx.tx_world += 1;
        let result = self.xp_import_document(inst, schema);
        self.tx.tx_world -= 1;
        match result {
            Ok(()) if guarded => {
                // The import's own transaction ends with it, so a second guarded import is not
                // blocked by the first one's rows
                // (xmlport/TestXmlPortImportWriteTx.al:GuardedImport_LeavingImportedRows_DoesNotBlockTheNextGuardedImport).
                if self.tx_model != TxModel::AutoRollback {
                    self.commit_transaction();
                }
                Ok(Value::Boolean(true))
            }
            Ok(()) => {
                if began_world {
                    self.commit_transaction();
                }
                Ok(Value::Null)
            }
            // A guarded import that fails returns false and loses its writes; the error stays
            // readable (xmlport/TestXmlPortImportWriteTx.al:StaticGuardedImport_FailingSecondElement_ReturnsFalse_AndRollsBackTheFirstRow).
            Err(Signal::Error(e)) if guarded => {
                self.rollback_transaction();
                self.error_finals = None;
                self.last_error = *e;
                Ok(Value::Boolean(false))
            }
            Err(other) => Err(other),
        }
    }

    fn xp_import_document(&mut self, inst: &Inst, schema: &Schema) -> Result<(), Signal> {
        let xp = schema.xp();
        let text = self.xp_read_source(inst, xp)?;
        // xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_MalformedXml_Throws
        if text.is_empty() {
            return Err(xerr(
                "Cannot import the specified XML document because the file is empty.",
                "XmlPortData",
            ));
        }
        let tops: Vec<usize> = (0..xp.nodes.len()).filter(|&i| xp.nodes[i].parent.is_none()).collect();
        let [top] = tops[..] else {
            return unsupported("XmlPort import with more than one top-level element (not pinned)");
        };
        // An import of a text port whose root is a table element is refused by BC, an export is not
        // (xmlport/lt-xmlport-query-datatransfer/XpTVariableText.al:Csv_ImportBasic and the other Csv_Import tests).
        if xp.format != ir::XmlFormat::Xml && xp.nodes[top].kind == ir::XmlNodeKind::Table {
            return Err(xerr(
                "If you choose XMLport format Fixed Text or Variable Text, then the root element must have source type Text.",
                "XmlPortInvalidNodeType",
            ));
        }
        if xp.nodes[top].kind != ir::XmlNodeKind::Text || schema.children(top).is_empty() {
            return unsupported("XmlPort import whose root is not a text element with content (not pinned)");
        }
        // A table view filters the rows the xmlport reads; what it does to an import is not pinned.
        for n in xp.nodes.iter().filter(|n| n.kind == ir::XmlNodeKind::Table) {
            if !self.xp_record(inst, n)?.borrow().filters.is_empty() {
                return unsupported("XmlPort import into a table element with a table view filter (not pinned)");
            }
        }
        if xp.format != ir::XmlFormat::Xml {
            return self.xp_import_text(inst, schema, top, &text);
        }
        let root = match al_builtins::xmlport::parse(&text) {
            Ok(r) => r,
            Err(ReadFailure::Malformed(Some(message))) => return Err(xerr(message, "XmlPortImport:Xml")),
            Err(ReadFailure::Malformed(None)) => {
                return unsupported("XmlPort import of a malformed document whose error text is not pinned");
            }
            Err(ReadFailure::Unsupported(why)) => return unsupported(format!("XmlPort import of {why}")),
        };
        if root.name != xp.nodes[top].xml_name {
            return Err(schema_error(format!("The '{}' element is not declared.", root.name)));
        }
        self.xp_import_container(inst, schema, top, &root)
    }

    /// The text of the source stream from its read position, which moves to the end. A byte order
    /// mark decides the encoding, else the text is UTF-8; the stream's own encoding does not matter.
    fn xp_read_source(&mut self, inst: &Inst, xp: &ir::XmlPort) -> Result<String, Signal> {
        let Value::Library(_, h) = inst.borrow().globals[xp.source_slot].clone() else {
            return unsupported("XmlPort import without SetSource (a file dialog is not supported)");
        };
        let mut g = h.borrow_mut();
        let LibraryValue::InStream(st) = &mut *g else {
            return unsupported("XmlPort import without SetSource (a file dialog is not supported)");
        };
        let bytes = st.drain();
        if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
            let units: Vec<u16> = rest.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
            return Ok(String::from_utf16_lossy(&units));
        }
        let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        match std::str::from_utf8(rest) {
            // A document that declares utf-16 in bytes without a UTF-16 byte order mark cannot be read
            // (xmlport/lt-xmlport-query-datatransfer/XpTImportBasic.al:Import_PrologUtf16Declared).
            Ok(t)
                if t.starts_with("<?xml")
                    && t.split_once("?>").is_some_and(|(decl, _)| {
                        let d = decl.to_ascii_lowercase();
                        d.contains("encoding=\"utf-16\"") || d.contains("encoding='utf-16'")
                    }) =>
            {
                Err(xerr(
                    "There is no Unicode byte order mark. Cannot switch to Unicode.",
                    "XmlPortImport:Xml",
                ))
            }
            Ok(t) => Ok(t.to_string()),
            Err(_) => unsupported("XmlPort import of a source that is not UTF-8 or UTF-16 (not pinned)"),
        }
    }

    fn xp_run_trigger(&mut self, inst: &Inst, schema: &Schema, node: usize, name: &str) -> Result<(), Signal> {
        match schema.node(node).trigger(name) {
            Some(r) => {
                // `CurrentPath` in a trigger of an export is the path of the element; in an import and outside the triggers it
                // is empty (platform/lt-xmlport-session-navapp-cookie/P94XpPathFlow.al: Export_PathsInTriggers,
                // Import_PathsInTriggers).
                let exporting = inst.borrow().xp_text.iter().any(|(k, _)| *k == 6);
                if exporting {
                    let path = Self::xp_node_path(schema, node);
                    let mut s = inst.borrow_mut();
                    s.xp_text.retain(|(k, _)| *k != 7);
                    s.xp_text.push((7, path));
                }
                {
                    let mut s = inst.borrow_mut();
                    s.xp_text.retain(|(k, _)| *k != 9);
                    s.xp_text.push((9, self.depth.to_string()));
                }
                let result = self.call(schema.object, r, inst.clone(), Vec::new()).map(|_| ());
                inst.borrow_mut().xp_text.retain(|(k, _)| *k != 7 && *k != 9);
                result
            }
            None => Ok(()),
        }
    }

    /// What the trigger that just ended asked for with Break, BreakUnbound or Quit.
    fn xp_flow(inst: &Inst) -> Option<Flow> {
        let s = inst.borrow();
        match s.xp_text.iter().find(|(k, _)| *k == 8).map(|(_, v)| v.as_str()) {
            Some("1") => Some(Flow::Break),
            Some("2") => Some(Flow::Unbound),
            Some("3") => Some(Flow::Quit),
            _ => None,
        }
    }

    fn xp_clear_flow(inst: &Inst) {
        inst.borrow_mut().xp_text.retain(|(k, _)| *k != 8);
    }

    /// `/Root/Item`, `/Root/Item/Id` and `/Root/Item[@Qty]`: the names of the elements above the node, an attribute last in brackets.
    fn xp_node_path(schema: &Schema, node: usize) -> String {
        let mut names = Vec::new();
        let mut at = Some(node);
        while let Some(n) = at {
            let nd = schema.node(n);
            let attr = matches!(nd.kind, ir::XmlNodeKind::TextAttr | ir::XmlNodeKind::FieldAttr);
            names.push((nd.xml_name.clone(), attr));
            at = nd.parent;
        }
        names.reverse();
        let mut out = String::new();
        for (name, attr) in names {
            if attr {
                out.push_str(&format!("[@{name}]"));
            } else {
                out.push('/');
                out.push_str(&name);
            }
        }
        out
    }

    /// The child of schema node `parent` that reads element `name` (`want_attr`: attribute).
    fn xp_child_named(schema: &Schema, parent: usize, name: &str, attr: bool) -> Option<usize> {
        schema.children(parent).into_iter().find(|&c| {
            let n = schema.node(c);
            let is_attr = matches!(n.kind, ir::XmlNodeKind::TextAttr | ir::XmlNodeKind::FieldAttr);
            is_attr == attr && n.xml_name == name
        })
    }

    fn xp_import_container(&mut self, inst: &Inst, schema: &Schema, node: usize, elem: &Elem) -> Result<(), Signal> {
        self.xp_import_attrs(inst, schema, node, elem, None)?;
        let mut seq = Sequence::new(schema, node);
        let mut linked: Vec<usize> = Vec::new();
        for ce in elem.elements() {
            let child = seq.accept(schema, &ce.name)?;
            match schema.node(child).kind {
                ir::XmlNodeKind::Table => {
                    let first = !linked.contains(&child);
                    linked.push(child);
                    self.xp_import_record(inst, schema, child, ce, first)?
                }
                ir::XmlNodeKind::Text if !schema.children(child).is_empty() => {
                    self.xp_import_container(inst, schema, child, ce)?
                }
                ir::XmlNodeKind::Text => self.xp_assign_variable(inst, schema, child, ce.text())?,
                _ => return unsupported("XmlPort import of a field element outside a table element"),
            }
        }
        seq.finish(schema)
    }

    /// Reads the attributes of `elem` into the schema's attribute nodes under `node`.
    /// `assigned` collects the table fields they set.
    fn xp_import_attrs(
        &mut self,
        inst: &Inst,
        schema: &Schema,
        node: usize,
        elem: &Elem,
        mut assigned: Option<&mut Vec<usize>>,
    ) -> Result<(), Signal> {
        // A field attribute of the schema has to be there (xmlport/lt-xmlport-query-datatransfer/XpTImportOptions.al:
        // Import_AttributesMissing, Import_AttributesAsElements); the first one the element lacks is named. For a text
        // attribute this is not pinned.
        for a in schema.children(node) {
            let n = schema.node(a);
            if n.kind == ir::XmlNodeKind::FieldAttr && !elem.attrs.iter().any(|(name, _)| *name == n.xml_name) {
                return Err(schema_error(format!(
                    "The required attribute '{}' is missing.",
                    n.xml_name
                )));
            }
        }
        for (name, value) in &elem.attrs {
            let Some(a) = Self::xp_child_named(schema, node, name, true) else {
                return unsupported(format!(
                    "XmlPort import of an attribute {name} the schema does not have (error not pinned)"
                ));
            };
            match schema.node(a).kind {
                ir::XmlNodeKind::TextAttr => self.xp_assign_variable(inst, schema, a, value.clone())?,
                _ => {
                    let pos = self.xp_assign_field(inst, schema, a, value)?;
                    if let Some(list) = assigned.as_deref_mut() {
                        list.push(pos);
                    }
                }
            }
        }
        Ok(())
    }

    /// Text element or attribute: the text goes into its variable, then `OnAfterAssignVariable`
    /// (xmlport/TestXmlPortAdvanced.al:XmlPort_TextVariableTriggers_ImportMutatesAssignedValues).
    fn xp_assign_variable(&mut self, inst: &Inst, schema: &Schema, node: usize, text: String) -> Result<(), Signal> {
        let slot = schema.node(node).slot.expect("text nodes have a slot");
        inst.borrow_mut().globals[slot] = Value::Text(text.into());
        self.xp_run_trigger(inst, schema, node, "onafterassignvariable")
    }

    /// Field element or attribute: the text becomes the field's value, then `OnAfterAssignField`.
    /// Returns the field's position.
    fn xp_assign_field(&mut self, inst: &Inst, schema: &Schema, node: usize, text: &str) -> Result<usize, Signal> {
        // White space around a value is dropped, for text too (xmlport/lt-xmlport-query-datatransfer/XpTImportBasic.al:
        // Import_WhitespaceAroundValues).
        let text = text.trim();
        let n = schema.node(node);
        let table_node = schema.node(n.table_node.expect("field nodes have a table node"));
        let pos = n.field.expect("field nodes have a field");
        let rec = self.xp_record(inst, table_node)?;
        let ty = {
            let r = rec.borrow();
            match &self.engine.tables[r.table as usize] {
                Some(t) => t.fields[pos].ty.clone(),
                None => return unsupported("XmlPort import into a table without a schema"),
            }
        };
        let value = if schema.xp().format_evaluate_xml {
            let members = {
                let r = rec.borrow();
                match &self.engine.tables[r.table as usize] {
                    Some(t) => self.members_of_field(t, pos)?,
                    None => None,
                }
            };
            self.xp_value_xml(text, &ty, members.map(|(_, m)| m))?
        } else if !schema.xp().format_evaluate_declared {
            // Without a FormatEvaluate the display text is read and, when that is no value, the XML form.
            match self.xp_value(text, &ty, false) {
                Ok(v) => v,
                Err(Signal::Error(first)) => match self.xp_value_xml(text, &ty, None) {
                    Ok(v) => v,
                    // A Date that neither reads gives the long text with the code Evaluate (XpTImportBasic.al: Import_BadDate,
                    // Import_DateWrongFormat).
                    Err(Signal::Error(_)) if matches!(ty, RtType::Date) => {
                        return Err(Signal::Error(Box::new(al_builtins::native::AlError {
                            message: format!(
                                "The format of the Date '{text}' does not match your device's date settings.\\\\Make sure that the day, month, and year are in the correct order and use the correct separator (such as - or /). Alternatively, you can always use the format 'yyyy-mm-dd'."
                            ),
                            code: "Evaluate".into(),
                            call_stack: String::new(),
                            ..Default::default()
                        })));
                    }
                    Err(Signal::Error(_)) => return Err(Signal::Error(first)),
                    Err(other) => return Err(other),
                },
                Err(other) => return Err(other),
            }
        } else {
            self.xp_value(text, &ty, false)?
        };
        rec.borrow_mut().fields[pos] = value;
        self.xp_run_trigger(inst, schema, node, "onafterassignfield")?;
        Ok(pos)
    }

    /// The value an imported text becomes in a field of type `ty`. An empty text is the type's
    /// default (xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_EmptyIntegerElement_ImportsZero,
    /// and an export writes a blank date as an empty element).
    fn xp_value(&mut self, text: &str, ty: &RtType, xml: bool) -> Result<Value, Signal> {
        if xml {
            return self.xp_value_xml(text, ty, None);
        }
        // A Legacy port reads the display text only: the XML date is refused and nothing is stored
        // (xmlport/TestXmlPortFormatEvaluateContracts.al:Import_LegacyXmlPort_TakesTheDisplayText_AndRefusesTheXmlDate).
        if matches!(ty, RtType::Date)
            && al_builtins::iso::parse_date(text).is_some()
            && text.as_bytes().get(4) == Some(&b'-')
        {
            return Err(xerr("The date is not valid.", "DateInvalid"));
        }
        match ty {
            RtType::Text(len) | RtType::Code(len) => {
                let n = text.encode_utf16().count();
                if let Some(m) = len.filter(|m| n > *m as usize) {
                    // xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_ValueTooLongForField_Throws
                    return Err(xerr(
                        format!(
                            "The length of the string is {n}, but it must be less than or equal to {m} characters. Value: {}",
                            ops::shown_in_length_error(text)
                        ),
                        "StringLengthExceeded",
                    ));
                }
                Ok(ops::convert(Value::Text(text.to_string().into()), ty)?)
            }
            RtType::Integer | RtType::BigInteger => {
                if text.is_empty() {
                    return Ok(self.default_value(ty));
                }
                let digits = text.strip_prefix('-').unwrap_or(text);
                let ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
                match (ty, ok.then(|| text.parse::<i64>().ok()).flatten()) {
                    (RtType::Integer, Some(n)) if i32::try_from(n).is_ok() => Ok(Value::Integer(n as i32)),
                    (RtType::BigInteger, Some(n)) => Ok(Value::BigInteger(n)),
                    _ => Err(evaluate_error(
                        text,
                        if *ty == RtType::Integer {
                            "Integer"
                        } else {
                            "BigInteger"
                        },
                    )),
                }
            }
            RtType::DateTime if text.ends_with(" AM") || text.ends_with(" PM") => {
                // The display text an export writes: `03/05/24 02:30 PM`
                // (xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_AcceptsTheTextItExports).
                let Some((date, clock)) = text.split_once(' ') else {
                    return Err(evaluate_error(text, "DateTime"));
                };
                let Value::Date(day, _) = self.xp_value(date, &RtType::Date, false)? else {
                    return Err(evaluate_error(text, "DateTime"));
                };
                let (hm, ampm) = clock.rsplit_once(' ').unwrap_or((clock, ""));
                // The display text of a DateTime with or without seconds (`03/05/24 12:34:56 PM` is what the
                // Legacy probe imports, xmlport/TestXmlPortFormatEvaluateContracts.al:167).
                let clock = if hm.matches(':').count() == 2 {
                    format!("{hm} {ampm}")
                } else {
                    format!("{hm}:00 {ampm}")
                };
                let ms = parse_time_12h(&clock).ok_or_else(|| evaluate_error(text, "DateTime"))?;
                Ok(Value::DateTime((i64::from(day) - 1) * 86_400_000 + ms + 1))
            }
            RtType::Decimal | RtType::Date | RtType::DateTime | RtType::Boolean => {
                if text.is_empty() && *ty != RtType::Boolean {
                    return Ok(self.default_value(ty));
                }
                let name = match ty {
                    RtType::Decimal => "Decimal",
                    RtType::Date => "Date",
                    RtType::DateTime => "DateTime",
                    _ => "Boolean",
                };
                let mut args = [self.default_value(ty), Value::Text(text.to_string().into())];
                self.ret_used = true;
                let ok = al_builtins::evaluate::evaluate(self, None, &mut args)?;
                if matches!(ok, Value::Boolean(true)) {
                    Ok(args[0].clone())
                } else {
                    Err(evaluate_error(text, name))
                }
            }
            RtType::Time => {
                if text.trim().is_empty() {
                    return Ok(Value::Time(0));
                }
                parse_time_12h(text.trim())
                    .map(|ms| Value::Time(ms + 1))
                    .ok_or_else(|| evaluate_error(text, "Time"))
            }
            RtType::Duration => {
                if text.trim().is_empty() {
                    return Ok(Value::Duration(0));
                }
                parse_duration_text(text)
                    .map(Value::Duration)
                    .ok_or_else(|| evaluate_error(text, "Duration"))
            }
            RtType::Option(members) => {
                let t = text.trim();
                let found = members.iter().position(|m| m.eq_ignore_ascii_case(t));
                match found {
                    Some(i) => Ok(Value::Option(i as i32, members.clone().into())),
                    None if t.is_empty() => Ok(Value::Option(0, members.clone().into())),
                    None => Err(evaluate_error(text, "Option")),
                }
            }
            RtType::Enum(id) => {
                let members = self.engine.program.objects[*id as usize].enum_values.clone();
                let t = text.trim();
                match members.iter().find(|(_, n)| n.eq_ignore_ascii_case(t)) {
                    Some((o, _)) => Ok(Value::Enum(*id, *o)),
                    None if t.is_empty() => Ok(Value::Enum(*id, members.first().map_or(0, |m| m.0))),
                    None => Err(evaluate_error(text, "Enum")),
                }
            }
            other => unsupported(format!(
                "XmlPort import into a field of type {} (not pinned)",
                other.describe()
            )),
        }
    }

    /// The value an imported text becomes under `FormatEvaluate = Xml`: the XML forms only
    /// (xmlport/TestXmlPortFormatEvaluateContracts.al:Import_XmlPort_TakesTheXmlForms_AndRefusesTheDisplayText).
    /// A Date that is not `yyyy-mm-dd` fails with the beginning of BC's text; the rest of that text is not
    /// pinned (docs/probes-needed.md). Display text for another type, and the types the probe does not
    /// cover, are refused.
    fn xp_value_xml(
        &mut self,
        text: &str,
        ty: &RtType,
        members: Option<Vec<(i32, String, String)>>,
    ) -> Result<Value, Signal> {
        match ty {
            // corpus xmlport/PWXmlPortFormatEvaluate.al:Import_Xml_Time: `hh:mm:ss` with optional
            // milliseconds; the 12-hour display text is refused.
            RtType::Time if text.is_empty() => Ok(Value::Time(0)),
            RtType::Time => al_builtins::iso::parse_time(text)
                .map(Value::Time)
                .ok_or_else(|| evaluate_error(text, "Time")),
            // Import_Xml_Duration: the ISO 8601 form (`PT1H`); milliseconds and words are refused.
            RtType::Duration if text.is_empty() => Ok(Value::Duration(0)),
            RtType::Duration => parse_iso_duration(text)
                .map(Value::Duration)
                .ok_or_else(|| evaluate_error(text, "Duration")),
            // Import_Xml_Option, Import_Xml_Enum: the member name, its ordinal or its caption.
            RtType::Option(names) => {
                let t = text.trim();
                if t.is_empty() {
                    return Ok(Value::Option(0, names.clone().into()));
                }
                let by_name = names.iter().position(|m| m.eq_ignore_ascii_case(t));
                let by_caption = members
                    .as_ref()
                    .and_then(|m| m.iter().position(|(_, _, c)| c.eq_ignore_ascii_case(t)));
                let by_ordinal = t.parse::<usize>().ok().filter(|i| *i < names.len());
                match by_name.or(by_ordinal).or(by_caption) {
                    Some(i) => Ok(Value::Option(i as i32, names.clone().into())),
                    None => Err(evaluate_error(text, "Option")),
                }
            }
            RtType::Enum(id) => {
                let t = text.trim();
                let list = match members {
                    Some(m) => m,
                    None => self.engine.program.objects[*id as usize]
                        .enum_values
                        .iter()
                        .map(|(o, n)| (*o, n.clone(), n.clone()))
                        .collect(),
                };
                if t.is_empty() {
                    return Ok(Value::Enum(*id, list.first().map_or(0, |m| m.0)));
                }
                let by_name = list.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(t));
                let by_ordinal = t
                    .parse::<i32>()
                    .ok()
                    .and_then(|o| list.iter().find(|(v, _, _)| *v == o));
                let by_caption = list.iter().find(|(_, _, c)| c.eq_ignore_ascii_case(t));
                match by_name.or(by_ordinal).or(by_caption) {
                    Some((o, _, _)) => Ok(Value::Enum(*id, *o)),
                    None => Err(evaluate_error(text, "Enum")),
                }
            }
            RtType::Text(_) | RtType::Code(_) | RtType::Integer | RtType::BigInteger => self.xp_value(text, ty, false),
            // An empty element is the type's default, a Boolean's too (XpTImportBasic.al:Import_EmptyValuesExplicit).
            RtType::Date | RtType::DateTime | RtType::Decimal | RtType::Boolean if text.is_empty() => {
                Ok(self.default_value(ty))
            }
            RtType::Date => {
                match al_builtins::iso::parse_date(text).filter(|_| text.as_bytes().get(4) == Some(&b'-')) {
                    Some(d) => Ok(Value::Date(d, false)),
                    None => Err(xerr(
                        format!("The format of the Date '{text}' does not match your device's date settings."),
                        "DateInvalid",
                    )),
                }
            }
            RtType::DateTime => {
                match al_builtins::iso::parse_datetime(text).filter(|_| text.as_bytes().get(4) == Some(&b'-')) {
                    Some(v) => Ok(Value::DateTime(v)),
                    None => Err(evaluate_error(text, "DateTime")),
                }
            }
            RtType::Decimal => {
                let plain = {
                    let digits = text.strip_prefix('-').unwrap_or(text);
                    !digits.is_empty()
                        && digits.split('.').count() <= 2
                        && digits.split('.').all(|p| p.bytes().all(|b| b.is_ascii_digit()))
                        && !digits.starts_with('.')
                };
                match text.parse::<rust_decimal::Decimal>() {
                    Ok(d) if plain => Ok(Value::Decimal(d)),
                    _ => Err(evaluate_error(text, "Decimal")),
                }
            }
            // Import_Xml_DisplayTextOfDecimalBooleanDateTime: `true`, `false` and `1`, `0`; `Yes` is refused.
            RtType::Boolean => match text {
                "true" | "1" => Ok(Value::Boolean(true)),
                "false" | "0" => Ok(Value::Boolean(false)),
                _ => Err(evaluate_error(text, "Boolean")),
            },
            other => unsupported(format!(
                "XmlPort import into a field of type {} with FormatEvaluate = Xml (the XML form is not pinned)",
                other.describe()
            )),
        }
    }

    /// One element of a table element: a fresh record, its link fields, `OnAfterInitRecord`,
    /// the attributes and children, and the save. `first_of_node` is false for the second and
    /// later elements of a table element under the same parent: only the first gets the link
    /// (xmlport/TestXmlPortTriggersReplaceViewContracts.al:Nested_Export_GroupsTheChildrenUnderTheirParent_ImportLinksOnlyTheFirstChild,
    /// corpus: .
    fn xp_import_record(
        &mut self,
        inst: &Inst,
        schema: &Schema,
        node: usize,
        elem: &Elem,
        first_of_node: bool,
    ) -> Result<(), Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, n)?;
        let table = rec.borrow().table as usize;
        let fresh = self.initial_fields(table);
        rec.borrow_mut().fields = fresh;
        let mut assigned: Vec<usize> = Vec::new();
        // Nested table elements take their link fields from the record above
        // (xmlport/TestXmlPortAdvanced.al:XmlPort_NestedTableElements_ImportCreatesParentAndChildren).
        if let Some(link) = n.link_node.filter(|_| first_of_node) {
            let parent = self.xp_record(inst, schema.node(link))?;
            for &(own, other) in &n.link_fields {
                let v = parent.borrow().fields[other].clone();
                let ty = self.engine.tables[table]
                    .as_ref()
                    .expect("a table element has a schema")
                    .fields[own]
                    .ty
                    .clone();
                rec.borrow_mut().fields[own] = al_builtins::ops::convert(v, &ty)?;
                assigned.push(own);
            }
        }
        self.xp_run_trigger(inst, schema, node, "onafterinitrecord")?;
        self.xp_import_attrs(inst, schema, node, elem, Some(&mut assigned))?;
        let mut saved = false;
        let mut seq = Sequence::new(schema, node);
        let mut linked: Vec<usize> = Vec::new();
        for ce in elem.elements() {
            let child = seq.accept(schema, &ce.name)?;
            match schema.node(child).kind {
                ir::XmlNodeKind::Field => {
                    let pos = self.xp_assign_field(inst, schema, child, &ce.text())?;
                    assigned.push(pos);
                }
                ir::XmlNodeKind::Text if schema.children(child).is_empty() => {
                    self.xp_assign_variable(inst, schema, child, ce.text())?
                }
                ir::XmlNodeKind::Table => {
                    // The record is stored before the elements below it are read (assumption:
                    // docs/probes-needed.md).
                    if !saved {
                        self.xp_save_record(inst, schema, node, &assigned)?;
                        saved = true;
                    }
                    let first = !linked.contains(&child);
                    linked.push(child);
                    self.xp_import_record(inst, schema, child, ce, first)?;
                }
                _ => return unsupported("XmlPort import of a text element with content inside a table element"),
            }
        }
        seq.finish(schema)?;
        if !saved {
            self.xp_save_record(inst, schema, node, &assigned)?;
        }
        Ok(())
    }

    /// Stores the buffer of table element `node`: it is inserted with its triggers and events, as
    /// `Insert(true)` does (xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_RunsTheTableInsertTriggersAndEvents).
    /// A row with the same key is modified when `AutoUpdate` or `AutoReplace` is on: `AutoReplace`
    /// replaces the whole buffer and `AutoUpdate` changes only the fields the document carries, and
    /// both keep the row's SystemId
    /// (xmlport/TestXmlPortTriggersReplaceViewContracts.al:Import_AutoReplace_ReplacesTheWholeBuffer_KeepsSystemId,
    /// :Import_AutoUpdate_ChangesOnlyTheCarriedFields_KeepsSystemId, corpus: .
    /// The `OnBefore...Record` and `OnAfter...Record` triggers bracket it, and `AutoSave = false`
    /// leaves the storing to them (xmlport/TestXmlPortAdvanced.al:XmlPort_AutoSaveFalse_...).
    fn xp_save_record(&mut self, inst: &Inst, schema: &Schema, node: usize, assigned: &[usize]) -> Result<(), Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, n)?;
        let t = self.table(&rec)?;
        let cx = self.cx.clone();
        let pk = t.primary_key(&rec.borrow().fields);
        let stored = self
            .store_for(&rec)
            .get(&cx, &t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e}")))?;
        let modify = stored.is_some() && (n.auto_update || n.auto_replace);
        let (before, after) = if modify {
            ("onbeforemodifyrecord", "onaftermodifyrecord")
        } else {
            ("onbeforeinsertrecord", "onafterinsertrecord")
        };
        self.xp_run_trigger(inst, schema, node, before)?;
        if n.auto_save {
            if modify && !n.auto_replace {
                // AutoUpdate changes only the fields the document carries.
                let has_assign_trigger = n.trigger("onafterinitrecord").is_some()
                    || schema.children(node).iter().any(|&c| {
                        let child = schema.node(c);
                        child.trigger("onafterassignfield").is_some()
                            || child.trigger("onafterassignvariable").is_some()
                    });
                if has_assign_trigger {
                    return unsupported("AutoUpdate with triggers that assign fields (which values win is not pinned)");
                }
                let imported = rec.borrow().fields.clone();
                let key_args: Vec<Value> = pk.clone();
                self.ret_used = true;
                self.record_op(RecordOp::Get, &rec, &mut key_args.clone())?;
                {
                    let mut r = rec.borrow_mut();
                    for &p in assigned {
                        r.fields[p] = imported[p].clone();
                    }
                }
            } else if modify {
                // The row keeps its system fields.
                let old = stored.expect("modify implies a stored row");
                let mut r = rec.borrow_mut();
                for (i, f) in t.fields.iter().enumerate() {
                    if f.id >= 2_000_000_000 {
                        r.fields[i] = old[i].clone();
                    }
                }
            }
            // A duplicate key raises the table's own error, as an AL Insert does.
            self.ret_used = false;
            let method = if modify { RecordOp::Modify } else { RecordOp::Insert };
            self.record_op(method, &rec, &mut [Value::Boolean(true)])?;
        }
        self.xp_run_trigger(inst, schema, node, after)
    }

    // ------------------------------------------------- VariableText, FixedText

    /// The root and table element of a text-format port: `Root > tableelement > fieldelements`.
    fn xp_text_layout(&self, schema: &Schema, top: usize) -> Result<(usize, Vec<usize>), Signal> {
        // A text-format port may have its table element at the top, with no root element
        // (P94Fixtures.al: "LT P94 Csv Plain", exported by P94XpDelim.al:Export_NothingDeclared).
        let direct = schema.node(top).kind == ir::XmlNodeKind::Table;
        let kids = if direct { vec![top] } else { schema.children(top) };
        let [table] = kids[..] else {
            return unsupported(
                "a VariableText or FixedText port whose root has other than one table element (not pinned)",
            );
        };
        if schema.node(table).kind != ir::XmlNodeKind::Table {
            return unsupported("a VariableText or FixedText port whose root has no table element (not pinned)");
        }
        let fields = schema.children(table);
        if fields.iter().any(|&f| schema.node(f).kind != ir::XmlNodeKind::Field) || fields.is_empty() {
            return unsupported(
                "a VariableText or FixedText port whose table element holds more than field elements (not pinned)",
            );
        }
        if schema.xp().format == ir::XmlFormat::FixedText && fields.iter().any(|&f| schema.node(f).width.is_none()) {
            return unsupported("a FixedText field element without a Width");
        }
        Ok((table, fields))
    }

    fn xp_import_text(&mut self, inst: &Inst, schema: &Schema, top: usize, text: &str) -> Result<(), Signal> {
        let xp = schema.xp();
        let (table, fields) = self.xp_text_layout(schema, top)?;
        let record_sep = non_empty(text_separator(
            set_or_declared(inst, 2, xp.record_separator.as_deref()).as_deref(),
            "\r\n",
        )?)?;
        let variable = xp.format == ir::XmlFormat::VariableText;
        let (fsep, fdelim) = (
            non_empty(text_separator(
                set_or_declared(inst, 1, xp.field_separator.as_deref()).as_deref(),
                ",",
            )?)?,
            text_separator(set_or_declared(inst, 0, xp.field_delimiter.as_deref()).as_deref(), "\"")?,
        );
        for line in text.split(record_sep.as_str()).filter(|l| !l.is_empty()) {
            let values: Vec<String> = if variable {
                split_variable(line, &fsep, &fdelim)
            } else {
                let chars: Vec<char> = line.chars().collect();
                let mut at = 0;
                fields
                    .iter()
                    .map(|&f| {
                        let w = schema.node(f).width.expect("checked by xp_text_layout");
                        let end = (at + w).min(chars.len());
                        let v: String = chars[at.min(end)..end].iter().collect();
                        at = end;
                        v
                    })
                    .collect()
            };
            let n = schema.node(table);
            let rec = self.xp_record(inst, n)?;
            let tbl = rec.borrow().table as usize;
            let fresh = self.initial_fields(tbl);
            rec.borrow_mut().fields = fresh;
            self.xp_run_trigger(inst, schema, table, "onafterinitrecord")?;
            let mut assigned = Vec::new();
            for (&f, v) in fields.iter().zip(&values) {
                let is_text = {
                    let pos = schema.node(f).field.expect("field nodes have a field");
                    matches!(
                        self.engine.tables[tbl].as_ref().map(|t| &t.fields[pos].ty),
                        Some(RtType::Text(_) | RtType::Code(_))
                    )
                };
                // A fixed-width field is padded with spaces.
                let v = if variable {
                    v.as_str()
                } else if is_text {
                    v.trim_end()
                } else {
                    v.trim()
                };
                assigned.push(self.xp_assign_field(inst, schema, f, v)?);
            }
            // Fields are read as the values come; a record of another length is not pinned.
            if values.len() != fields.len() {
                return unsupported(format!(
                    "a record with {} values for a port of {} fields (not pinned)",
                    values.len(),
                    fields.len()
                ));
            }
            self.xp_save_record(inst, schema, table, &assigned)?;
        }
        Ok(())
    }

    fn xp_export_text(&mut self, inst: &Inst, schema: &Schema, top: usize) -> Result<Vec<u8>, Signal> {
        let xp = schema.xp();
        let (table, fields) = self.xp_text_layout(schema, top)?;
        let record_sep = text_separator(
            set_or_declared(inst, 2, xp.record_separator.as_deref()).as_deref(),
            "\r\n",
        )?;
        let variable = xp.format == ir::XmlFormat::VariableText;
        let (fsep, fdelim) = (
            text_separator(set_or_declared(inst, 1, xp.field_separator.as_deref()).as_deref(), ",")?,
            text_separator(set_or_declared(inst, 0, xp.field_delimiter.as_deref()).as_deref(), "\"")?,
        );
        let mut lines = Vec::new();
        for_each_row(self, inst, schema, table, |me, inst, schema| {
            let mut cells = Vec::new();
            for &f in &fields {
                let v = me.xp_field_text(inst, schema, f)?;
                cells.push(if variable {
                    format!("{fdelim}{v}{fdelim}")
                } else {
                    let w = schema.node(f).width.expect("checked by xp_text_layout");
                    let mut cut: String = v.chars().take(w).collect();
                    while cut.chars().count() < w {
                        cut.push(' ');
                    }
                    cut
                });
            }
            lines.push(cells.join(if variable { fsep.as_str() } else { "" }));
            Ok(())
        })?;
        let text = lines.join(&record_sep);
        if xp.utf8 {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(text.as_bytes());
            Ok(out)
        } else if text.is_ascii() {
            Ok(text.into_bytes())
        } else {
            unsupported(
                "a VariableText or FixedText export with characters beyond ASCII and no TextEncoding (not pinned)",
            )
        }
    }

    // ---------------------------------------------------------------- export

    fn xp_export(&mut self, inst: &Inst, schema: &Schema, guarded: bool) -> Result<Value, Signal> {
        inst.borrow_mut().xp_text.push((6, String::new()));
        let result = self.xp_export_inner(inst, schema, guarded);
        inst.borrow_mut().xp_text.retain(|(k, _)| *k != 6 && *k != 7);
        result
    }

    fn xp_export_inner(&mut self, inst: &Inst, schema: &Schema, guarded: bool) -> Result<Value, Signal> {
        let xp = schema.xp();
        // The document is written in its own encoding whatever the stream's own.
        let blob = match inst.borrow().globals[xp.destination_slot].clone() {
            Value::Library(_, h) => match &*h.borrow() {
                LibraryValue::OutStream(blob, _) => blob.clone(),
                _ => return unsupported("XmlPort export without SetDestination (a download is not supported)"),
            },
            _ => return unsupported("XmlPort export without SetDestination (a download is not supported)"),
        };
        if xp.nodes.iter().any(|n| n.use_temporary) {
            return unsupported("XmlPort export from a UseTemporary table element (it needs OnPreXmlItem; not pinned)");
        }
        let tops: Vec<usize> = (0..xp.nodes.len()).filter(|&i| xp.nodes[i].parent.is_none()).collect();
        let [top] = tops[..] else {
            return unsupported("XmlPort export with more than one top-level element (not pinned)");
        };
        let table_top = xp.format != ir::XmlFormat::Xml && xp.nodes[top].kind == ir::XmlNodeKind::Table;
        if !table_top && (xp.nodes[top].kind != ir::XmlNodeKind::Text || schema.children(top).is_empty()) {
            return unsupported("XmlPort export whose root is not a text element with content (not pinned)");
        }
        let bytes = if xp.format == ir::XmlFormat::Xml {
            if xp.utf8 {
                return unsupported("XmlPort export with TextEncoding = UTF8 (not pinned)");
            }
            match self.xp_export_container(inst, schema, top) {
                Ok(root) => al_builtins::xmlport::utf16_bom_bytes(&al_builtins::xmlport::serialize(&root)),
                Err(Signal::Error(_)) if guarded => {
                    return unsupported("an error inside an Export whose result is used (not pinned)");
                }
                Err(e) => return Err(e),
            }
        } else {
            self.xp_export_text(inst, schema, top)?
        };
        al_builtins::stream::write_bytes(&blob, &bytes);
        Ok(if guarded { Value::Boolean(true) } else { Value::Null })
    }

    /// A text element with content: its attributes, then its children in schema order.
    fn xp_export_container(&mut self, inst: &Inst, schema: &Schema, node: usize) -> Result<Elem, Signal> {
        let mut out = Elem::new(&schema.node(node).xml_name);
        self.xp_export_content(inst, schema, node, &mut out)?;
        Ok(out)
    }

    fn xp_export_content(&mut self, inst: &Inst, schema: &Schema, node: usize, out: &mut Elem) -> Result<(), Signal> {
        for child in schema.children(node) {
            let c = schema.node(child);
            match c.kind {
                ir::XmlNodeKind::TextAttr => {
                    let v = self.xp_pass_variable(inst, schema, child)?;
                    out.attrs.push((c.xml_name.clone(), v));
                }
                ir::XmlNodeKind::FieldAttr => {
                    let v = self.xp_field_text(inst, schema, child)?;
                    out.attrs.push((c.xml_name.clone(), v));
                }
                ir::XmlNodeKind::Field => {
                    let v = self.xp_field_text(inst, schema, child)?;
                    let mut e = Elem::new(&c.xml_name);
                    e.children.push(Node::Text(v));
                    out.children.push(Node::Elem(e));
                }
                ir::XmlNodeKind::Text if schema.children(child).is_empty() => {
                    let v = self.xp_pass_variable(inst, schema, child)?;
                    let mut e = Elem::new(&c.xml_name);
                    e.children.push(Node::Text(v));
                    out.children.push(Node::Elem(e));
                }
                ir::XmlNodeKind::Text if schema.node(node).kind == ir::XmlNodeKind::Table => {
                    return unsupported("XmlPort export of a text element with content inside a table element");
                }
                ir::XmlNodeKind::Text => {
                    let e = self.xp_export_container(inst, schema, child)?;
                    out.children.push(Node::Elem(e));
                }
                ir::XmlNodeKind::Table => {
                    for e in self.xp_export_table(inst, schema, child)? {
                        out.children.push(Node::Elem(e));
                    }
                }
            }
        }
        Ok(())
    }

    /// `OnBeforePassVariable`, then the variable's text
    /// (xmlport/TestXmlPortAdvanced.al:XmlPort_TextVariableTriggers_ExportManipulatesTextAndAttributes).
    fn xp_pass_variable(&mut self, inst: &Inst, schema: &Schema, node: usize) -> Result<String, Signal> {
        self.xp_run_trigger(inst, schema, node, "onbeforepassvariable")?;
        let slot = schema.node(node).slot.expect("text nodes have a slot");
        match &inst.borrow().globals[slot] {
            Value::Text(s) | Value::Code(s) => Ok(s.to_string()),
            other => unsupported(format!("XmlPort export of the variable {}", other.describe())),
        }
    }

    /// `OnBeforePassField` (it runs for every row: xmlport/TestXmlPortTriggersReplaceViewContracts.al:
    /// Export_OnBeforePassField_Runs_AndSetTableViewFiltersAndOrders), then the field's text: the
    /// display text of its type
    /// (xmlport/TestXmlPortExportImportContracts.al:XmlPortExport_Document_FullText). A blank option
    /// is a single space.
    fn xp_field_text(&mut self, inst: &Inst, schema: &Schema, node: usize) -> Result<String, Signal> {
        self.xp_run_trigger(inst, schema, node, "onbeforepassfield")?;
        let n = schema.node(node);
        let rec = self.xp_record(inst, schema.node(n.table_node.expect("field nodes have a table node")))?;
        let pos = n.field.expect("field nodes have a field");
        // A FlowField is calculated for the export (assumption xmlport-exports-flowfield-calculated).
        if !self.table(&rec)?.fields[pos].stored {
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::XmlportExportsFlowfieldCalculated);
            self.ret_used = false;
            self.record_op(RecordOp::CalcFields, &rec, &mut [Value::Integer(pos as i32)])?;
        }
        let v = rec.borrow().fields[pos].clone();
        if schema.xp().format_evaluate_xml {
            return self.xp_field_text_xml(&v);
        }
        // An Option field with an OptionCaption writes the caption (corpus
        // xmlport/PWXmlPortFormatEvaluate.al:Export_Legacy_TimeDurationOptionEnum: `Beta cap`).
        if let Value::Option(o, _) = &v {
            let t = self.table(&rec)?;
            if let Some((_, members)) = self.members_of_field(&t, pos)? {
                if let Some((_, _, caption)) = members.get(*o as usize) {
                    return Ok(if caption.is_empty() {
                        " ".to_string()
                    } else {
                        caption.clone()
                    });
                }
            }
        }
        match &v {
            Value::Integer(i) => Ok(i.to_string()),
            Value::Text(s) | Value::Code(s) => Ok(s.to_string()),
            Value::Decimal(_) => al_builtins::xmlport::field_text(&v)
                .or_else(|why| unsupported(format!("XmlPort export of {why} (the text form is not pinned)"))),
            Value::Boolean(_) | Value::Date(..) | Value::Time(_) | Value::DateTime(_) | Value::Duration(_) => {
                al_builtins::format::try_format(&v, 0, 0, self)
                    .or_else(|why| unsupported(format!("XmlPort export of a value: {why}")))
            }
            Value::Option(..) | Value::Enum(..) => {
                let t = al_builtins::format::try_format(&v, 0, 0, self)
                    .or_else(|why| unsupported(format!("XmlPort export of a value: {why}")))?;
                Ok(if t.is_empty() { " ".to_string() } else { t })
            }
            other => unsupported(format!(
                "XmlPort export of {} (the text form is not pinned)",
                other.describe()
            )),
        }
    }

    /// The XML forms `FormatEvaluate = Xml` writes (xmlport/TestXmlPortFormatEvaluateContracts.al:
    /// Export_Xml_WritesTheXmlForms): `2024-03-05`, `1234.50`, `true`, `2024-03-05T12:34:56Z`. Other
    /// types are not pinned.
    fn xp_field_text_xml(&mut self, v: &Value) -> Result<String, Signal> {
        match v {
            Value::Integer(i) => Ok(i.to_string()),
            Value::Text(s) | Value::Code(s) => Ok(s.to_string()),
            Value::Decimal(_) => al_builtins::xmlport::field_text(v)
                .map(|t| t.replace(',', ""))
                .or_else(|why| unsupported(format!("XmlPort export of {why} (the text form is not pinned)"))),
            Value::Boolean(b) => Ok(if *b { "true" } else { "false" }.to_string()),
            Value::Date(..) | Value::DateTime(_) => al_builtins::format::try_format(v, 0, 9, self)
                .or_else(|why| unsupported(format!("XmlPort export of a value: {why}"))),
            // corpus xmlport/PWXmlPortFormatEvaluate.al:Export_Xml_TimeDurationOptionEnum: `12:34:56`,
            // `P1DT1H1M1.001S`, and the ordinal of an Option or Enum.
            Value::Time(0) | Value::Duration(0) => Ok(String::new()),
            Value::Time(t) => {
                let ms = t - 1;
                if ms % 1000 != 0 {
                    return unsupported(
                        "XmlPort export of a Time with milliseconds with FormatEvaluate = Xml (not pinned)",
                    );
                }
                let s = ms / 1000;
                Ok(format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60))
            }
            Value::Duration(d) => Ok(iso_duration(*d)),
            Value::Option(i, _) => Ok(i.to_string()),
            Value::Enum(_, o) => Ok(o.to_string()),
            other => unsupported(format!(
                "XmlPort export of {} with FormatEvaluate = Xml (the XML form is not pinned)",
                other.describe()
            )),
        }
    }

    /// One element per row of the table element, in the order of the table view (primary key
    /// order by default), after the table view and the link to the record above
    /// (xmlport/TestXmlPortAdvanced.al:XmlPort_NestedTableElements_ExportIncludesChildRecordsAndAttribute).
    fn xp_export_table(&mut self, inst: &Inst, schema: &Schema, node: usize) -> Result<Vec<Elem>, Signal> {
        let n = schema.node(node);
        let mut out = Vec::new();
        let mut rows = Vec::new();
        self.xp_run_trigger(inst, schema, node, "onprexmlitem")?;
        self.xp_rows(inst, schema, node, &mut rows)?;
        let rec = self.xp_record(inst, n)?;
        // A table element without rows still passes its fields once, without a row: OnBeforePassField runs and no
        // OnAfterGetRecord (P94XpPathFlow.al:Export_PathsNoRows). Unless it is optional, the element is written once with the
        // values of the record as it stands (xmlport/lt-xmlport-query-datatransfer/XpTExportBasic.al:Export_StaticWithRecordNoRows,
        // Export_EmptyTable: 462 bytes, a row of `0`, empty text, `0.00`, `No`).
        if rows.is_empty() && !n.min_zero {
            let mut e = Elem::new(&n.xml_name);
            self.xp_export_content(inst, schema, node, &mut e)?;
            out.push(e);
        } else if rows.is_empty() {
            for child in schema.children(node) {
                if matches!(
                    schema.node(child).kind,
                    ir::XmlNodeKind::Field | ir::XmlNodeKind::FieldAttr
                ) {
                    self.xp_run_trigger(inst, schema, child, "onbeforepassfield")?;
                }
            }
        }
        for row in rows {
            self.load(&rec, row)?;
            self.xp_run_trigger(inst, schema, node, "onaftergetrecord")?;
            match Self::xp_flow(inst) {
                Some(Flow::Break) => {
                    Self::xp_clear_flow(inst);
                    break;
                }
                Some(Flow::Unbound) => {
                    Self::xp_clear_flow(inst);
                    continue;
                }
                Some(Flow::Quit) => return Err(Signal::Exit(None)),
                None => {}
            }
            let mut e = Elem::new(&n.xml_name);
            self.xp_export_content(inst, schema, node, &mut e)?;
            out.push(e);
        }
        Ok(out)
    }

    /// The rows of table element `node` in export order.
    fn xp_rows(&mut self, inst: &Inst, schema: &Schema, node: usize, rows: &mut Vec<Vec<Value>>) -> Result<(), Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, n)?;
        let t = self.table(&rec)?;
        if let Some(link) = n.link_node {
            let parent = self.xp_record(inst, schema.node(link))?;
            for &(own, other) in &n.link_fields {
                let v = parent.borrow().fields[other].clone();
                self.ret_used = false;
                self.record_op(RecordOp::SetRange, &rec, &mut [Value::Integer(own as i32), v])?;
            }
        }
        let (key, asc) = {
            let r = rec.borrow();
            (r.key, r.ascending)
        };
        let mut after = None;
        while let Some(row) = self.scan(&rec, &t, asc, after.take())? {
            after = Some(position(&t, key, &row));
            rows.push(row);
        }
        Ok(())
    }
}

/// Runs `body` once for every row of table element `node`, with the row loaded.
/// What a trigger asked for: leave the table element's rows, skip the rest of the row, or end the run.
#[derive(Clone, Copy, PartialEq)]
enum Flow {
    Break,
    Unbound,
    Quit,
}

fn for_each_row(
    me: &mut Session,
    inst: &Inst,
    schema: &Schema,
    node: usize,
    mut body: impl FnMut(&mut Session, &Inst, &Schema) -> Result<(), Signal>,
) -> Result<(), Signal> {
    let mut rows = Vec::new();
    me.xp_run_trigger(inst, schema, node, "onprexmlitem")?;
    me.xp_rows(inst, schema, node, &mut rows)?;
    let rec = me.xp_record(inst, schema.node(node))?;
    for row in rows {
        me.load(&rec, row)?;
        me.xp_run_trigger(inst, schema, node, "onaftergetrecord")?;
        match Session::xp_flow(inst) {
            Some(Flow::Break) => {
                Session::xp_clear_flow(inst);
                break;
            }
            Some(Flow::Unbound) => {
                Session::xp_clear_flow(inst);
                continue;
            }
            Some(Flow::Quit) => return Err(Signal::Exit(None)),
            None => {}
        }
        body(me, inst, schema)?;
    }
    Ok(())
}

/// A separator property: `<LF>`, `<CR>` and `<TAB>` stand for the control characters, other text
/// is itself. Other `<...>` names are not pinned.
/// A declared separator as the getter shows it: `<None>` is empty, `<CR/LF>`, `<LF>`, `<CR>` and `<Tab>` are their escapes, anything
/// else is as written (P94XpDelim.al:Getters_Declared, Getters_DeclaredTwoTables). Other tokens are not pinned.
fn declared_separator(v: &str) -> Result<String, Signal> {
    Ok(match v.to_ascii_uppercase().as_str() {
        "<NONE>" => String::new(),
        "<CR/LF>" => "\\r\\n".into(),
        "<LF>" => "\\n".into(),
        "<CR>" => "\\r".into(),
        "<TAB>" => "\\t".into(),
        u if u.starts_with('<') && u.ends_with('>') => {
            return unsupported(format!("the separator token {v} read back (not pinned)"));
        }
        _ => v.to_string(),
    })
}

/// The text a setter stored for separator `kind` (0 delimiter, 1 field, 2 record, 3 table), else the declared one.
fn set_or_declared(inst: &Inst, kind: u8, declared: Option<&str>) -> Option<String> {
    let set = inst
        .borrow()
        .xp_text
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, v)| v.clone());
    set.or_else(|| declared.map(str::to_string))
}

/// A field or record separator has to hold text; an empty one is not pinned.
fn non_empty(s: String) -> Result<String, Signal> {
    if s.is_empty() {
        unsupported("an empty field or record separator (not pinned)")
    } else {
        Ok(s)
    }
}

fn text_separator(value: Option<&str>, default: &str) -> Result<String, Signal> {
    let Some(v) = value else { return Ok(default.to_string()) };
    let mut out = String::new();
    let mut rest = v;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        match rest[start + 1..start + end].to_ascii_uppercase().as_str() {
            "LF" => out.push('\n'),
            "CR" => out.push('\r'),
            "TAB" => out.push('\t'),
            // <NewLine> writes CR LF (XpTVariableText.al:Csv_ExportThreeRows reads the lines back as <NL>-separated).
            "NEWLINE" | "CR/LF" => out.push_str("\r\n"),
            // <None> is the empty text (XmlPortFixtures.al "LT XpT Csv Semi" has no field delimiter).
            "NONE" => {}
            other => return unsupported(format!("the separator token <{other}> (not pinned)")),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Splits a VariableText record at `sep`. A value that starts with the delimiter runs to the next
/// delimiter and may hold the separator; a doubled delimiter is not an escape, so the text after
/// the closing delimiter starts the next value
/// (xmlport/TestXmlPortTextFormatsContracts.al:VariableText_Import_DoubledQuoteInsideAQuotedValue_IsNotAnEscape,
/// corpus: .
fn split_variable(line: &str, sep: &str, delim: &str) -> Vec<String> {
    if delim.is_empty() {
        return line.split(sep).map(str::to_string).collect();
    }
    let mut out = Vec::new();
    let mut rest = line;
    loop {
        if let Some(after) = rest.strip_prefix(delim) {
            match after.find(delim) {
                Some(end) => {
                    out.push(after[..end].to_string());
                    rest = &after[end + delim.len()..];
                    match rest.strip_prefix(sep) {
                        Some(r) => rest = r,
                        None if rest.is_empty() => return out,
                        None => {}
                    }
                    if rest.is_empty() && !line.ends_with(sep) {
                        return out;
                    }
                }
                None => {
                    out.push(after.to_string());
                    return out;
                }
            }
        } else {
            match rest.find(sep) {
                Some(end) => {
                    out.push(rest[..end].to_string());
                    rest = &rest[end + sep.len()..];
                }
                None => {
                    out.push(rest.to_string());
                    return out;
                }
            }
        }
    }
}

/// `h:mm:ss AM` as milliseconds since midnight.
fn parse_time_12h(t: &str) -> Option<i64> {
    let (clock, ampm) = t.rsplit_once(' ')?;
    let mut it = clock.split(':');
    let (h, m, s): (i64, i64, i64) = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    if it.next().is_some() || !(1..=12).contains(&h) || m > 59 || s > 59 {
        return None;
    }
    let pm = match ampm {
        "AM" => false,
        "PM" => true,
        _ => return None,
    };
    Some((((h % 12) + if pm { 12 } else { 0 }) * 3600 + m * 60 + s) * 1000)
}

/// `1 day 1 hour 1 minute 1 second 1 millisecond` (the text an export writes) as milliseconds.
/// An ISO 8601 duration (`P1DT1H1M1.001S`, `PT1H`) in milliseconds.
fn parse_iso_duration(t: &str) -> Option<i64> {
    let rest = t.strip_prefix('P')?;
    let (days, time) = match rest.split_once('T') {
        Some((d, tm)) => (d, tm),
        None => (rest, ""),
    };
    if days.is_empty() && time.is_empty() {
        return None;
    }
    let mut total = 0i64;
    let mut take = |part: &str, units: &[(char, f64)]| -> Option<()> {
        let mut num = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                num.push(c);
                continue;
            }
            let mult = units.iter().find(|(u, _)| *u == c)?.1;
            let n: f64 = num.parse().ok()?;
            total += (n * mult).round() as i64;
            num.clear();
        }
        num.is_empty().then_some(())
    };
    take(days, &[('D', 86_400_000.0)])?;
    take(time, &[('H', 3_600_000.0), ('M', 60_000.0), ('S', 1000.0)])?;
    Some(total)
}

/// `P1DT1H1M1.001S` for 90061001 ms; zero parts are left out.
fn iso_duration(ms: i64) -> String {
    let (days, rest) = (ms / 86_400_000, ms % 86_400_000);
    let (h, rest) = (rest / 3_600_000, rest % 3_600_000);
    let (m, rest) = (rest / 60_000, rest % 60_000);
    let (sec, milli) = (rest / 1000, rest % 1000);
    let mut out = String::from("P");
    if days != 0 {
        out.push_str(&format!("{days}D"));
    }
    if h != 0 || m != 0 || rest != 0 {
        out.push('T');
        if h != 0 {
            out.push_str(&format!("{h}H"));
        }
        if m != 0 {
            out.push_str(&format!("{m}M"));
        }
        if rest != 0 {
            out.push_str(&if milli == 0 {
                format!("{sec}S")
            } else {
                format!("{sec}.{milli:03}S")
            });
        }
    }
    out
}

fn parse_duration_text(t: &str) -> Option<i64> {
    let mut total = 0i64;
    let mut words = t.split_whitespace();
    let mut any = false;
    while let Some(n) = words.next() {
        let n: i64 = n.parse().ok()?;
        let unit = words.next()?.trim_end_matches('s');
        total += n * match unit {
            "day" => 86_400_000,
            "hour" => 3_600_000,
            "minute" => 60_000,
            "second" => 1000,
            "millisecond" => 1,
            _ => return None,
        };
        any = true;
    }
    any.then_some(total)
}

/// Checks the child elements of an imported element against the schema's sequence: the children
/// in declaration order, each once (a table element any number of times) unless it is optional
/// (`MinOccurs = Zero`). The messages are .NET's schema validation messages
/// (xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_SchemaMismatch_ThrowsXmlPortInvalidXml).
struct Sequence {
    decl: Vec<usize>,
    parent: String,
    pos: usize,
    seen: usize,
    optional: Vec<bool>,
    repeat: Vec<bool>,
    names: Vec<String>,
}

impl Sequence {
    fn new(schema: &Schema, node: usize) -> Sequence {
        let decl: Vec<usize> = schema
            .children(node)
            .into_iter()
            .filter(|&c| {
                !matches!(
                    schema.node(c).kind,
                    ir::XmlNodeKind::TextAttr | ir::XmlNodeKind::FieldAttr
                )
            })
            .collect();
        Sequence {
            parent: schema.node(node).xml_name.clone(),
            pos: 0,
            seen: 0,
            optional: decl.iter().map(|&c| schema.node(c).min_zero).collect(),
            repeat: decl
                .iter()
                .map(|&c| schema.node(c).kind == ir::XmlNodeKind::Table)
                .collect(),
            names: decl.iter().map(|&c| schema.node(c).xml_name.clone()).collect(),
            decl,
        }
    }

    /// The names that may come next.
    fn expected(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.pos >= self.decl.len() {
            return out;
        }
        let done = self.seen >= 1 || self.optional[self.pos];
        if self.seen == 0 || self.repeat[self.pos] {
            out.push(self.names[self.pos].clone());
        }
        if done {
            for j in self.pos + 1..self.decl.len() {
                out.push(self.names[j].clone());
                if !self.optional[j] {
                    break;
                }
            }
        }
        out
    }

    fn list(&self, head: String) -> Result<String, Signal> {
        let expected = self.expected();
        match expected.len() {
            0 => Ok(head),
            1 => Ok(format!("{head} List of possible elements expected: '{}'.", expected[0])),
            _ => unsupported("a schema error that lists several possible elements (the list form is not pinned)"),
        }
    }

    /// The schema node that reads child element `name`.
    fn accept(&mut self, _schema: &Schema, name: &str) -> Result<usize, Signal> {
        if self.pos < self.decl.len() {
            let cur = self.pos;
            if self.names[cur] == name && (self.seen == 0 || self.repeat[cur]) {
                self.seen += 1;
                return Ok(self.decl[cur]);
            }
            if self.seen >= 1 || self.optional[cur] {
                for j in cur + 1..self.decl.len() {
                    if self.names[j] == name {
                        self.pos = j;
                        self.seen = 1;
                        return Ok(self.decl[j]);
                    }
                    if !self.optional[j] {
                        break;
                    }
                }
            }
        }
        Err(schema_error(self.list(format!(
            "The element '{}' has invalid child element '{name}'.",
            self.parent
        ))?))
    }

    fn finish(&self, _schema: &Schema) -> Result<(), Signal> {
        if self.decl.is_empty() {
            return Ok(());
        }
        let done = self.seen >= 1 || self.optional[self.pos];
        if done && (self.pos + 1..self.decl.len()).all(|j| self.optional[j]) {
            return Ok(());
        }
        Err(schema_error(self.list(format!(
            "The element '{}' has incomplete content.",
            self.parent
        ))?))
    }
}

#[cfg(test)]
mod tests {
    use super::{declared_separator, split_variable};

    // xmlport/TestXmlPortTextFormatsContracts.al:VariableText_Import_QuotedUnquotedAndBlankValues
    #[test]
    fn a_variable_text_record_splits_into_its_values() {
        let split = |l: &str| split_variable(l, ",", "\"");
        assert_eq!(
            split("\"1\",\"a,b\",\"1,234.50\",\"03/05/24\""),
            ["1", "a,b", "1,234.50", "03/05/24"]
        );
        assert_eq!(split("\"2\",\"plain\",\"0\",\"\""), ["2", "plain", "0", ""]);
        assert_eq!(split("3,unquoted,5.5,01/02/24"), ["3", "unquoted", "5.5", "01/02/24"]);
        // A doubled delimiter is no escape: the text after the closing one starts the next value.
        assert_eq!(
            split("\"2\",\"say \"\"hi\"\"\",\"0\",\"\""),
            ["2", "say ", "hi", "", "0", ""]
        );
    }

    // platform/lt-xmlport-session-navapp-cookie/P94XpDelim.al:Getters_Declared, Getters_DeclaredTwoTables
    #[test]
    fn a_declared_separator_reads_back_as_backslash_text() {
        assert_eq!(declared_separator("<CR/LF>").unwrap(), "\\r\\n");
        assert_eq!(declared_separator("<LF>").unwrap(), "\\n");
        assert_eq!(declared_separator("<None>").unwrap(), "");
        assert_eq!(declared_separator(";").unwrap(), ";");
        assert!(declared_separator("<Space>").is_err());
    }
}
