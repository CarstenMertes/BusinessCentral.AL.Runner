//! `TableRelation`: which table a field relates to (`FieldRef.Relation`) and whether a value
//! exists there (`Validate`).
//!
//! Pinned by corpus fieldref/TestFieldRefRelation.al and TestFieldRefRelationFieldClass.al: the
//! first arm whose conditions hold applies, an else arm has none, a condition on a FlowFilter
//! field is not evaluated, `where(A = field(B))` reads B off the referencing row, and a value the
//! related table lacks is refused with "... cannot be found in the related table".

use crate::Session;
use al_builtins::filter::{FieldFilter, FilterExpr};
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{RecordState, Value};
use al_data::{Store, TableDef};
use al_ir as ir;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

/// `SystemCreatedBy` relates to the `User` table (corpus record/TestFieldDataClassificationVirtualTable.al:92-100).
const USER_TABLE: i32 = 2_000_000_120;
const SYSTEM_CREATED_BY: u32 = 2_000_000_002;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

impl Session {
    fn is_flow_field(field: &ir::Field) -> bool {
        field
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("FieldClass") && v.trim().eq_ignore_ascii_case("FlowField"))
    }

    fn is_flow_filter(field: &ir::Field) -> bool {
        field
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("FieldClass") && v.trim().eq_ignore_ascii_case("FlowFilter"))
    }

    /// The arm of field `pos`'s relation that applies to the buffer, if any.
    fn relation_arm(&self, rec: &Rec, t: &TableDef, pos: usize) -> Result<Option<ir::RelationArm>, Signal> {
        let schema = self.engine.program.objects[t.object as usize]
            .table
            .as_ref()
            .ok_or_else(|| Signal::Unsupported("relation of a table without a schema".into()))?;
        let field = &schema.fields[pos];
        if !field.relation {
            return Ok(None);
        }
        // The arms with `Field = filter(x)` conditions are kept apart (`relation_runtime`) and apply here only.
        let arms = if field.relation_arms.is_empty() {
            &field.relation_runtime
        } else {
            &field.relation_arms
        };
        if arms.is_empty() {
            return unsupported(format!(
                "the TableRelation of {}.{} has a shape that is not understood yet",
                t.name, field.name
            ));
        }
        let r = rec.borrow();
        for arm in arms {
            let mut holds = true;
            for c in &arm.conditions {
                // BC evaluates conditions on Normal fields and FlowFields only
                // (corpus fieldref/TestFieldRefRelationFieldClass.al:106-145).
                if Self::is_flow_filter(&schema.fields[c.field]) {
                    continue;
                }
                let ok = if c.filter {
                    // `Field = filter(x)`: the arm applies when the field's value passes the filter, which reads
                    // like a SetFilter text (`filter('')` the blank value, `filter(<> '')` any other). Only the
                    // `const` form is pinned by the corpus; this one is an assumption (docs/probes-needed.md).
                    let al_ir::Const::Text(text) = &c.value else {
                        return unsupported("a filter condition of a TableRelation without its text");
                    };
                    let engine = self.engine.clone();
                    let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
                    let expr = al_builtins::filter::parse_filter_with(text, &[], &t.fields[c.field].ty, &enums)
                        .map_err(|e| {
                            Signal::Unsupported(format!(
                                "the filter condition `{text}` of the TableRelation of {}.{}: {e}",
                                t.name, field.name
                            ))
                        })?;
                    expr.matches(&r.fields[c.field])
                } else {
                    ops::compare(&r.fields[c.field], &crate::constant(&c.value)).ok() == Some(Ordering::Equal)
                };
                if !ok {
                    holds = false;
                    break;
                }
            }
            if holds {
                return Ok(Some(arm.clone()));
            }
        }
        Ok(None)
    }

    /// `FieldRef.Relation`: the id of the related table, 0 without a relation.
    pub(crate) fn field_relation(&self, rec: &Rec, t: &TableDef, pos: usize) -> Result<i32, Signal> {
        let id = t.fields[pos].id;
        if id >= 2_000_000_000 {
            return if id == SYSTEM_CREATED_BY {
                Ok(USER_TABLE)
            } else {
                unsupported(format!("the relation of system field {}", t.fields[pos].name))
            };
        }
        match self.relation_arm(rec, t, pos)? {
            None => Ok(0),
            Some(arm) => match arm.table {
                Some(table) => Ok(self.engine.program.objects[table].id.unwrap_or(0) as i32),
                None => unsupported(format!(
                    "relation target `{}` is not declared in the workspace",
                    arm.table_name
                )),
            },
        }
    }

    /// Before `Validate` assigns `value`: the related table must hold it, unless the field says
    /// `ValidateTableRelation = false` or the value is blank. A refused value is never stored
    /// (corpus record/TestCalcFormulaSystemFieldsTests.Codeunit.al:253-259).
    pub(crate) fn validate_relation(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        pos: usize,
        value: Value,
    ) -> Result<(), Signal> {
        let schema = self.engine.program.objects[t.object as usize]
            .table
            .clone()
            .expect("table");
        let field = &schema.fields[pos];
        if field
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("ValidateTableRelation") && v.trim().eq_ignore_ascii_case("false"))
        {
            return Ok(());
        }
        let Some(arm) = self.relation_arm(rec, t, pos)? else {
            return Ok(());
        };
        if crate::recmeta::is_blank(&value)? {
            return Ok(());
        }
        let Some(table) = arm.table else {
            return unsupported(format!(
                "relation target `{}` is not declared in the workspace",
                arm.table_name
            ));
        };
        let Some(wheres) = &arm.wheres else {
            return unsupported(format!(
                "the where() clause of the relation to `{}` is not understood yet",
                arm.table_name
            ));
        };
        let related = self.engine.tables[table]
            .clone()
            .ok_or_else(|| Signal::Unsupported("relation to a table without a schema".into()))?;
        let rel_pos = arm.field.unwrap_or(related.keys[0][0]);
        let mut filters = vec![FieldFilter {
            field: rel_pos,
            expr: FilterExpr::Eq(ops::convert(value.clone(), &related.fields[rel_pos].ty)?),
            group: 0,
        }];
        for w in wheres {
            filters.push(match w {
                ir::RelationWhere::Value { related: p, text } => FieldFilter {
                    field: *p,
                    expr: self
                        .parse_filter_text(text, &related.fields[*p].ty)
                        .map_err(|e| Signal::Unsupported(format!("where() of a relation: {e}")))?,
                    group: 0,
                },
                ir::RelationWhere::Field { own, .. } if Self::is_flow_field(&schema.fields[*own]) => {
                    // Whether the check calculates the FlowField first is not pinned; the value as it
                    // stands (no CalcFields) made a valid value fail (docs/probes-needed.md).
                    return unsupported(format!(
                        "the TableRelation of {}.{} filters by `field({})`, a FlowField of the same record: whether BC calculates it for the check is not pinned",
                        t.name, field.name, schema.fields[*own].name
                    ));
                }
                ir::RelationWhere::Field { related: p, own } => FieldFilter {
                    field: *p,
                    expr: FilterExpr::Eq(ops::convert(rec.borrow().fields[*own].clone(), &related.fields[*p].ty)?),
                    group: 0,
                },
            });
        }
        let cx = self.cx_for(rec, &related)?;
        let found = self
            .store
            .count(&cx, &related, &filters)
            .map_err(|e| Signal::Unsupported(format!("{e}")))?;
        if found > 0 {
            return Ok(());
        }
        // A related table the baseline could not fill may hold the row in BC.
        self.unseeded_read(&related, false, "the TableRelation check")?;
        Err(Signal::Error(AlError {
            message: format!(
                "The field {} of table {} contains a value ({}) that cannot be found in the related table ({}).",
                self.field_caption(t, pos),
                self.caption(t),
                self.message_value(&value)?,
                self.caption(&related)
            ),
            code: "DB:PrimRecordNotFound".into(),
            call_stack: String::new(),
        }))
    }
}
