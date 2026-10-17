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
    fn is_flow_field(&self, table: u32, pos: usize) -> bool {
        self.engine.field_facts[table as usize][pos].class == ir::FieldClass::FlowField
    }

    fn is_flow_filter(&self, table: u32, pos: usize) -> bool {
        self.engine.field_facts[table as usize][pos].class == ir::FieldClass::FlowFilter
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
                if self.is_flow_filter(t.object, c.field) {
                    continue;
                }
                let ok = if c.filter {
                    // `Field = filter(x)`: the arm applies when the field's value passes the filter, which reads
                    // like a SetFilter text (`filter('')` the blank value, `filter(<> '')` any other; corpus types/
                    // lt-filter-enum-relation-duration-guid/C_TableRelation.al: Zone_Blank_Arm_Loc and the `A|B`, `C` arms).
                    let al_ir::Const::Text(text) = &c.value else {
                        return unsupported("a filter condition of a TableRelation without its text");
                    };
                    // The AL string of `filter('A|B')` is the filter text `A|B`; the blank `filter('')` stays the blank value
                    // (query/lt-query-links-relations-httpstream/RelArms.al:P100_ZoneFilterArms).
                    let unquoted;
                    let text = match text.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
                        Some(inner) if !inner.is_empty() && !inner.replace("''", "").contains('\'') => {
                            unquoted = inner.replace("''", "'");
                            &unquoted
                        }
                        _ => text,
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
                for a in &arm.assumes {
                    al_builtins::assumptions::assume(*a);
                }
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
        // Typed field facts, worked out once (a scan of the field's properties on each Validate showed in the Tests-ERM profile).
        if !self.engine.field_facts[t.object as usize][pos].validate_relation {
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
            // A condition on a FlowFilter field of the related table is a flow filter: it selects no row. Not pinned;
            // Item Journal Line's "Bin Code" relation names the Bin's "Item Filter" and "Variant Filter"
            // (Tests-SCM Invt. Doc. Pst Preview TestInventoryShipmentPreviewWithBin; assumption
            // relation-where-on-a-flowfilter-of-the-target).
            let related_pos = match w {
                ir::RelationWhere::Value { related, .. } | ir::RelationWhere::Field { related, .. } => *related,
            };
            if self.is_flow_filter(related.object, related_pos) {
                // record/lt-relation-where-flowfilter/RelationWhereFlowFilter.al:ValidateFlowFilterConstRelation, ValidateFlowFilterFieldRelation_ExistingValue.
                continue;
            }
            filters.push(match w {
                ir::RelationWhere::Value { related: p, text } => FieldFilter {
                    field: *p,
                    expr: self
                        .parse_filter_text(text, &related.fields[*p].ty)
                        .map_err(|e| Signal::Unsupported(format!("where() of a relation: {e}")))?,
                    group: 0,
                },
                ir::RelationWhere::Field { related: p, own } => FieldFilter {
                    field: *p,
                    expr: FilterExpr::Eq({
                        // The check calculates a FlowField the filter names (staging types/PZMisc.al:
                        // Validate_TableRelationFilteredByAFlowField, BC 29.0). Whether the calculated value
                        // stays on the record is not observed (docs/probes-needed.md).
                        if self.is_flow_field(t.object, *own) {
                            self.calc_fields(rec, &[*own])?;
                        }
                        // A condition that names the validated field itself (`Item Variant.Code where("Item No." =
                        // field("Item No."), Code = field("Variant Code"))` on Prod. Order Line, Assembly Header and
                        // Assembly Line "Variant Code") reads the value being validated, not the old one; the old
                        // blank value would make the filter contradict the key. Pinned by record/lt-relation-where-validated/
                        // RelationWhereValidated.al (Validate_OldBlank_ExistingAndMissing, Validate_OldNonBlank_NewExisting).
                        let own_value = if *own == pos {
                            value.clone()
                        } else {
                            rec.borrow().fields[*own].clone()
                        };
                        ops::convert(own_value, &related.fields[*p].ty)?
                    }),
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
        Err(Signal::Error(Box::new(AlError {
            message: format!(
                "The field {} of table {} contains a value ({}) that cannot be found in the related table ({}).",
                self.field_caption(t, pos),
                self.caption(t),
                self.message_value(&value)?,
                self.caption(&related)
            ),
            // The related table "Field" (2000000041) answers DB:RecordNotFound (staging types/PZMisc.al:
            // Validate_TableRelation_MissingFieldNumber, BC 29.0); other tables DB:PrimRecordNotFound.
            // A relation with a where() filter answers DB:NothingInsideFilter, whether the value is missing or fails the filter
            // (query/lt-query-links-relations-httpstream/RelQuotedEnum.al:P100_Validate_EachKind).
            code: if self.engine.program.objects[related.object as usize].id == Some(2_000_000_041) {
                "DB:RecordNotFound".into()
            } else if let [ir::RelationWhere::Value { related: w, .. } | ir::RelationWhere::Field { related: w, .. }] =
                wheres.as_slice()
            {
                // A single where() on a field of the related table's primary key answers DB:RecordNotFound (corpus
                // misc/lt-probes-needed-6/PnF13RelationFilterConditions.al:122-126, 144-149, 166-169, primary key (Code, Zone)); on
                // another field DB:NothingInsideFilter (record/lt-relation-where-empty-term/RelationWhereEmptyTerm.al:173-177). Several conditions are refused.
                if related.keys[0].contains(w) {
                    "DB:RecordNotFound".into()
                } else {
                    "DB:NothingInsideFilter".into()
                }
            } else if !wheres.is_empty()
                && rel_pos_in_key(&related, arm.field)
                && wheres.iter().all(|w| match w {
                    ir::RelationWhere::Value { related: p, .. } | ir::RelationWhere::Field { related: p, .. } => {
                        related.keys[0].contains(p)
                    }
                })
            {
                // Several conditions, all on fields of the related table's primary key, over a related field of that key: DB:RecordNotFound
                // (record/lt-relation-where-validated/RelationWhereValidated.al: `missing|code`, `other-item|code`). A mix with a field
                // outside the key is not pinned.
                "DB:RecordNotFound".into()
            } else if !wheres.is_empty() {
                // Several conditions: which code BC answers is not pinned for the other mixes, so the error path is refused.
                return Err(Signal::Unsupported(
                    "the error code of a TableRelation check with several where() conditions (not pinned)".into(),
                ));
            } else {
                "DB:PrimRecordNotFound".into()
            },
            call_stack: String::new(),
            ..Default::default()
        })))
    }
}

/// Whether the related field of a relation (the first key field when none is named) is in the related table's primary key.
fn rel_pos_in_key(related: &al_data::TableDef, field: Option<usize>) -> bool {
    let pos = field.unwrap_or(related.keys[0][0]);
    related.keys[0].contains(&pos)
}
