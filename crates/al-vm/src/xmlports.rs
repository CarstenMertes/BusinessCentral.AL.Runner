//! XmlPort import and export (Format = Xml): `SetSource`, `SetDestination`, `SetTableView`,
//! `Import` and `Export`, instance and static forms.
//!
//! Every rule is pinned by a corpus test, cited where it is applied. What no test pins is
//! refused (`Signal::Unsupported`) and listed in docs/probes-needed.md: the exact bytes of an
//! export (declaration, whitespace), the texts of import errors, field types other than
//! Integer, Decimal, Text and Code, tables with triggers or event subscribers, and so on.

use crate::records::Rec;
use crate::transactions::{TRANSACTION_STOPPED, TxModel, al_error};
use crate::{AlError, Session, Signal};
use al_builtins::stream::Encoding;
use al_builtins::value::{Instance, LibraryValue, Value};
use al_builtins::xmlport::{Elem, FieldFailure, Node, ReadFailure};
use al_data::{Store, position};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type Inst = Rc<RefCell<Instance>>;

/// Code of the error an import raises when its document does not fit the xmlport. BC's text is
/// not pinned, so the message says so; the code lets the statement form refuse instead.
const IMPORT_FAILURE: &str = "AL-RUNTIME:XmlPortImport";

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn import_failure<T>(what: impl AsRef<str>) -> Result<T, Signal> {
    Err(Signal::Error(AlError {
        message: format!(
            "XmlPort import failed (the message text is not pinned): {}",
            what.as_ref()
        ),
        code: IMPORT_FAILURE.into(),
        call_stack: String::new(),
    }))
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
        id: &str,
        receiver: Option<Value>,
        args: &[Value],
        ret_used: bool,
    ) -> Result<Value, Signal> {
        match id {
            "Xmlport.SetSource#1" | "Xmlport.SetDestination#1" => {
                let inst = self.xp_instance(receiver)?;
                let schema = self.xp_schema(&inst)?;
                let (slot, want) = if id == "Xmlport.SetSource#1" {
                    (schema.xp().source_slot, "InStream")
                } else {
                    (schema.xp().destination_slot, "OutStream")
                };
                match args.first() {
                    Some(v @ Value::Library(n, _)) if n.eq_ignore_ascii_case(want) => {
                        inst.borrow_mut().globals[slot] = v.clone();
                        Ok(Value::Null)
                    }
                    other => unsupported(format!("{id} with {other:?}")),
                }
            }
            "Xmlport.SetTableView#1" => {
                let inst = self.xp_instance(receiver)?;
                self.xp_set_view(&inst, args.first())?;
                Ok(Value::Null)
            }
            "Xmlport.Export#1" | "Xmlport.Import#1" => {
                let inst = self.xp_instance(receiver)?;
                let dir = if id.starts_with("Xmlport.Export") {
                    Dir::Export
                } else {
                    Dir::Import
                };
                self.xp_run(&inst, dir, ret_used, false)
            }
            // The static forms run a fresh instance of the xmlport with that id.
            "Xmlport.Export#2" | "Xmlport.Import#2" => {
                let dir = if id.starts_with("Xmlport.Export") {
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
                    .position(|o| o.kind == "xmlport" && o.id == u32::try_from(*n).ok())
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
                    other => return unsupported(format!("{id} with the stream {other:?}")),
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
            other => unsupported(format!("an XmlPort method on {other:?}")),
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
            other => unsupported(format!("table element {} holds {other:?}", node.name)),
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
            return unsupported(format!("SetTableView with {view:?}"));
        };
        let target = self.xp_record(inst, first)?;
        let v = view.borrow();
        if v.table != target.borrow().table {
            return unsupported("SetTableView with a record of another table (error text not pinned)");
        }
        if v.temporary || v.marked_only || v.key != 0 || !v.ascending {
            return unsupported("SetTableView with a temporary, marked or re-sorted record (not pinned)");
        }
        let mut t = target.borrow_mut();
        t.filters = v.filters.clone();
        t.filter_group = v.filter_group;
        Ok(())
    }

    fn xp_run(&mut self, inst: &Inst, dir: Dir, ret_used: bool, static_form: bool) -> Result<Value, Signal> {
        let schema = self.xp_schema(inst)?;
        let declared = schema.xp().direction;
        let refused = match (dir, declared) {
            (Dir::Import, ir::XmlDirection::Export) => Some(("Import", "Export")),
            (Dir::Export, ir::XmlDirection::Import) => Some(("Export", "Import")),
            _ => None,
        };
        if let Some((what, has)) = refused {
            // The platform refuses with a message the corpus does not pin
            // (xmlport/TestXmlPortPrecompiledObject.al only checks that it is not empty).
            return unsupported(format!(
                "{what} of XmlPort \"{}\", which declares Direction = {has} (error text not pinned)",
                schema.name()
            ));
        }
        match dir {
            Dir::Export => self.xp_export(inst, &schema, ret_used),
            Dir::Import => self.xp_import(inst, &schema, ret_used, static_form),
        }
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
                    Err(al_error(TRANSACTION_STOPPED))
                };
            }
            if self.try_depth > 0 {
                return unsupported("guarded XmlPort.Import inside a TryFunction (not pinned)");
            }
        }
        // Import begins a transaction of its own, so a write is allowed inside it even under
        // TransactionModel::None, and the transaction ends with the import
        // (codeunit/TestWriteTxTestBoundary.al:WriteTx_Test12_AnXmlPortImportMayWriteUnderNone).
        let began_world = self.no_transaction();
        self.tx_world += 1;
        let result = self.xp_import_document(inst, schema);
        self.tx_world -= 1;
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
            // A guarded import that fails returns false and loses its writes
            // (xmlport/TestXmlPortImportWriteTx.al:StaticGuardedImport_FailingSecondElement_ReturnsFalse_AndRollsBackTheFirstRow).
            Err(Signal::Error(e)) if guarded => {
                self.rollback_transaction();
                self.error_finals = None;
                // An error of ours has no BC text to give; an error raised by AL code inside a
                // trigger keeps its own.
                self.last_error = if e.code == IMPORT_FAILURE {
                    al_builtins::native::unpinned_error()
                } else {
                    e
                };
                Ok(Value::Boolean(false))
            }
            Err(Signal::Error(e)) if e.code == IMPORT_FAILURE => unsupported(format!(
                "a failing XmlPort import used as a statement: its error text is not pinned ({})",
                e.message
            )),
            Err(other) => Err(other),
        }
    }

    fn xp_import_document(&mut self, inst: &Inst, schema: &Schema) -> Result<(), Signal> {
        let xp = schema.xp();
        let text = self.xp_read_source(inst, xp)?;
        let root = match al_builtins::xmlport::parse(&text) {
            Ok(r) => r,
            Err(ReadFailure::Malformed) => return import_failure("the document is not well-formed XML"),
            Err(ReadFailure::Unsupported(why)) => return unsupported(format!("XmlPort import of {why}")),
        };
        let tops: Vec<usize> = (0..xp.nodes.len()).filter(|&i| xp.nodes[i].parent.is_none()).collect();
        let [top] = tops[..] else {
            return unsupported("XmlPort import with more than one top-level element (not pinned)");
        };
        if xp.nodes[top].kind != ir::XmlNodeKind::Text || schema.children(top).is_empty() {
            return unsupported("XmlPort import whose root is not a text element with content (not pinned)");
        }
        // A table view filters the rows the xmlport reads; what it does to an import is not pinned.
        for n in xp.nodes.iter().filter(|n| n.kind == ir::XmlNodeKind::Table) {
            if !self.xp_record(inst, n)?.borrow().filters.is_empty() {
                return unsupported("XmlPort import into a table element with a table view filter (not pinned)");
            }
        }
        if root.name != xp.nodes[top].xml_name {
            return import_failure(format!(
                "the root element is <{}>, the schema expects <{}>",
                root.name, xp.nodes[top].xml_name
            ));
        }
        self.xp_import_container(inst, schema, top, &root)
    }

    /// The text of the source stream from its read position, which moves to the end.
    fn xp_read_source(&mut self, inst: &Inst, xp: &ir::XmlPort) -> Result<String, Signal> {
        let Value::Library(_, h) = inst.borrow().globals[xp.source_slot].clone() else {
            return unsupported("XmlPort import without SetSource (a file dialog is not supported)");
        };
        let mut g = h.borrow_mut();
        let LibraryValue::InStream(st) = &mut *g else {
            return unsupported("XmlPort import without SetSource (a file dialog is not supported)");
        };
        if !matches!(st.enc, Encoding::Utf8 | Encoding::Windows) {
            return unsupported("XmlPort import from a stream that is not UTF-8 (not pinned)");
        }
        let bytes = st.drain();
        match String::from_utf8(bytes) {
            Ok(t) => Ok(t),
            Err(_) => import_failure("the document is not valid UTF-8"),
        }
    }

    fn xp_run_trigger(&mut self, inst: &Inst, schema: &Schema, node: usize, name: &str) -> Result<(), Signal> {
        match schema.node(node).trigger(name) {
            Some(r) => self.call(schema.object, r, inst.clone(), Vec::new()).map(|_| ()),
            None => Ok(()),
        }
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
        for ce in elem.elements() {
            let Some(child) = Self::xp_child_named(schema, node, &ce.name, false) else {
                return unsupported(format!(
                    "XmlPort import of an element <{}> the schema does not have (error not pinned)",
                    ce.name
                ));
            };
            match schema.node(child).kind {
                ir::XmlNodeKind::Table => self.xp_import_record(inst, schema, child, ce)?,
                ir::XmlNodeKind::Text if !schema.children(child).is_empty() => {
                    self.xp_import_container(inst, schema, child, ce)?
                }
                ir::XmlNodeKind::Text => self.xp_assign_variable(inst, schema, child, ce.text())?,
                _ => return unsupported("XmlPort import of a field element outside a table element"),
            }
        }
        Ok(())
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
        inst.borrow_mut().globals[slot] = Value::Text(text);
        self.xp_run_trigger(inst, schema, node, "onafterassignvariable")
    }

    /// Field element or attribute: the text becomes the field's value, then `OnAfterAssignField`.
    /// Returns the field's position.
    fn xp_assign_field(&mut self, inst: &Inst, schema: &Schema, node: usize, text: &str) -> Result<usize, Signal> {
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
        let value = match al_builtins::xmlport::field_value(text, &ty) {
            Ok(v) => v,
            Err(FieldFailure::Invalid(why)) => return import_failure(format!("field {}: {why}", n.name)),
            Err(FieldFailure::Unsupported(why)) => {
                return unsupported(format!("XmlPort import into field {} of {why} (not pinned)", n.name));
            }
        };
        rec.borrow_mut().fields[pos] = value;
        self.xp_run_trigger(inst, schema, node, "onafterassignfield")?;
        Ok(pos)
    }

    /// One element of a table element: a fresh record, its link fields, `OnAfterInitRecord`,
    /// the attributes and children, and the save.
    fn xp_import_record(&mut self, inst: &Inst, schema: &Schema, node: usize, elem: &Elem) -> Result<(), Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, n)?;
        let table = rec.borrow().table as usize;
        let fresh = self.initial_fields(table);
        rec.borrow_mut().fields = fresh;
        let mut assigned: Vec<usize> = Vec::new();
        // Nested table elements take their link fields from the record above
        // (xmlport/TestXmlPortAdvanced.al:XmlPort_NestedTableElements_ImportCreatesParentAndChildren).
        if let Some(link) = n.link_node {
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
        for ce in elem.elements() {
            let Some(child) = Self::xp_child_named(schema, node, &ce.name, false) else {
                return unsupported(format!(
                    "XmlPort import of an element <{}> the schema does not have (error not pinned)",
                    ce.name
                ));
            };
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
                    self.xp_import_record(inst, schema, child, ce)?;
                }
                _ => return unsupported("XmlPort import of a text element with content inside a table element"),
            }
        }
        if !saved {
            self.xp_save_record(inst, schema, node, &assigned)?;
        }
        Ok(())
    }

    /// Stores the buffer of table element `node`. A row with the same key is modified when
    /// `AutoUpdate` or `AutoReplace` is on, else the buffer is inserted; the matching
    /// `OnBefore...Record` and `OnAfter...Record` triggers bracket it, and `AutoSave = false`
    /// leaves the storing to them
    /// (xmlport/TestXmlPortAdvanced.al:XmlPort_AutoSaveFalse_OnBeforeAndAfterInsertControlPersistence,
    /// :..ModifyControlPersistence, :XmlPort_AutoUpdate_..., :XmlPort_AutoReplace_...).
    fn xp_save_record(&mut self, inst: &Inst, schema: &Schema, node: usize, assigned: &[usize]) -> Result<(), Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, n)?;
        let t = self.table(&rec)?;
        if self.has_write_hooks(&t)? {
            return unsupported(format!(
                "XmlPort import into {}, which has insert or modify triggers or subscribers (not pinned)",
                t.name
            ));
        }
        let cx = self.cx.clone();
        let pk = t.primary_key(&rec.borrow().fields);
        let stored = self
            .store_for(&rec)
            .get(&cx, &t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
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
                self.record_op("Get", &rec, &mut key_args.clone())?;
                {
                    let mut r = rec.borrow_mut();
                    for &p in assigned {
                        r.fields[p] = imported[p].clone();
                    }
                }
            }
            // A duplicate key raises the table's own error, as an AL Insert does.
            self.ret_used = false;
            let method = if modify { "Modify" } else { "Insert" };
            self.record_op(method, &rec, &mut [Value::Boolean(false)])?;
        }
        self.xp_run_trigger(inst, schema, node, after)
    }

    // ---------------------------------------------------------------- export

    fn xp_export(&mut self, inst: &Inst, schema: &Schema, guarded: bool) -> Result<Value, Signal> {
        let xp = schema.xp();
        let blob = match inst.borrow().globals[xp.destination_slot].clone() {
            Value::Library(_, h) => match &*h.borrow() {
                // A stream without an encoding argument is Windows-1252 now; the document is written in
                // its own encoding whatever the stream's (the owner of xmlports to revisit).
                LibraryValue::OutStream(blob, Encoding::Utf8 | Encoding::Windows) => blob.clone(),
                LibraryValue::OutStream(..) => {
                    return unsupported("XmlPort export to a stream that is not UTF-8 (not pinned)");
                }
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
        if xp.nodes[top].kind != ir::XmlNodeKind::Text || schema.children(top).is_empty() {
            return unsupported("XmlPort export whose root is not a text element with content (not pinned)");
        }
        let result = self.xp_export_container(inst, schema, top);
        let root = match result {
            Ok(r) => r,
            Err(Signal::Error(_)) if guarded => {
                return unsupported("an error inside an Export whose result is used (not pinned)");
            }
            Err(e) => return Err(e),
        };
        let text = match al_builtins::xmlport::serialize(&root) {
            Ok(t) => t,
            Err(e) => {
                return unsupported(format!(
                    "XmlPort export of the empty element <{}> (<X/> or <X></X> is not pinned)",
                    e.0
                ));
            }
        };
        al_builtins::stream::write_bytes(&blob, text.as_bytes());
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
            Value::Text(s) | Value::Code(s) => Ok(s.clone()),
            other => unsupported(format!("XmlPort export of the variable {other:?}")),
        }
    }

    fn xp_field_text(&mut self, inst: &Inst, schema: &Schema, node: usize) -> Result<String, Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, schema.node(n.table_node.expect("field nodes have a table node")))?;
        let v = rec.borrow().fields[n.field.expect("field nodes have a field")].clone();
        al_builtins::xmlport::field_text(&v)
            .or_else(|why| unsupported(format!("XmlPort export of {why} (the text form is not pinned)")))
    }

    /// One element per row of the table element, in primary key order, after the table view and
    /// the link to the record above
    /// (xmlport/TestXmlPortAdvanced.al:XmlPort_NestedTableElements_ExportIncludesChildRecordsAndAttribute).
    fn xp_export_table(&mut self, inst: &Inst, schema: &Schema, node: usize) -> Result<Vec<Elem>, Signal> {
        let n = schema.node(node);
        let rec = self.xp_record(inst, n)?;
        let t = self.table(&rec)?;
        if let Some(link) = n.link_node {
            let parent = self.xp_record(inst, schema.node(link))?;
            for &(own, other) in &n.link_fields {
                let v = parent.borrow().fields[other].clone();
                self.ret_used = false;
                self.record_op("SetRange", &rec, &mut [Value::Integer(own as i32), v])?;
            }
        }
        let key = rec.borrow().key;
        let mut rows = Vec::new();
        let mut after = None;
        while let Some(row) = self.scan(&rec, &t, true, after.take())? {
            after = Some(position(&t, key, &row));
            rows.push(row);
        }
        let mut out = Vec::new();
        for row in rows {
            self.load(&rec, row)?;
            let mut e = Elem::new(&n.xml_name);
            self.xp_export_content(inst, schema, node, &mut e)?;
            out.push(e);
        }
        Ok(out)
    }
}
