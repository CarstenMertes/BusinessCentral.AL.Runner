//! The rows of a restored company, from a SQL Server backup (`.bak`) or a BC cloud export
//! (`.bacpac`), as `alrunner test --test-data` reads them (ADR-020 point 3).
//!
//! The provider does no naming of its own beyond finding the database table: a table is its AL
//! name with SQL's character substitutions, selected by company and, when two apps declare the same
//! name, by the declaring app's id. Columns come back as the database names them (extension
//! columns already joined, `$ext` companions merged on their own key); the runtime maps them onto
//! the fields of the compiled program.

use crate::FileSource;
use al_bcdb::query::{self, BcTable, ReadRequest, plan_read_table};
use al_bcdb::source::BcSource;
use al_bcdb::symbols::{eq_ci, normalize_sql_name};
use al_bcdb::value::Value;
use al_host::testdata::{Cell, TableError, TableProvider, TableRequest, TableRows};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The company of Microsoft's demo database, and of the corpus's oracle.
pub const DEFAULT_COMPANY: &str = "CRONUS International Ltd.";

pub struct BackupProvider {
    src: Box<dyn BcSource>,
    tables: Vec<BcTable>,
    /// The company's name as it appears in table names (`CRONUS International Ltd_`).
    sql_company: String,
    /// The company's name as AL sees it (`CRONUS International Ltd.`).
    company: String,
    source: String,
}

fn cell(v: &Value) -> Cell {
    match v {
        Value::Null => Cell::Null,
        Value::Bool(b) => Cell::Bool(*b),
        Value::Int(i) => Cell::Int(*i),
        Value::Real(f) => Cell::Float(f64::from(*f)),
        Value::Float(f) => Cell::Float(*f),
        Value::Decimal {
            negative,
            magnitude,
            scale,
        } => Cell::Decimal {
            negative: *negative,
            magnitude: *magnitude,
            scale: *scale,
        },
        Value::DateTime { days, ticks } => Cell::DateTime {
            days: *days,
            ticks: *ticks,
        },
        Value::Date { days } => Cell::Date { days: *days },
        Value::Time { units, scale } => Cell::Time {
            units: *units,
            scale: *scale,
        },
        Value::DateTime2 { days, units, scale } => Cell::DateTime2 {
            days: *days,
            units: *units,
            scale: *scale,
        },
        Value::Guid(g) => Cell::Guid(*g),
        Value::Binary(b) => Cell::Binary(b.clone()),
        Value::RowVersion(v) => Cell::RowVersion(*v),
        Value::Text(s) => Cell::Text(s.clone()),
    }
}

impl BackupProvider {
    /// Opens a `.bak` or `.bacpac`. `company` names the company to restore (exact, then any case,
    /// then a prefix); without it the database's only company, or else the demo company
    /// [`DEFAULT_COMPANY`]. A database with several companies and none of them the demo company
    /// needs the name.
    pub fn open(path: &Path, company: Option<&str>) -> Result<BackupProvider, String> {
        let file = FileSource::open(path)?;
        let src = al_bcdb::open(Box::new(file)).map_err(|e| format!("{}: {e}", path.display()))?;
        let tables = query::bc_tables(&*src).map_err(|e| format!("{}: {e}", path.display()))?;
        let companies = query::companies(&*src).map_err(|e| format!("{}: {e}", path.display()))?;
        let file_name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let sql_company = pick_company(&companies, company, &file_name)?;
        let company = al_company_name(&*src, &tables, &sql_company);
        Ok(BackupProvider {
            src,
            tables,
            sql_company,
            company,
            source: file_name,
        })
    }

    /// The companies a database holds, in their SQL spelling.
    pub fn companies(path: &Path) -> Result<Vec<String>, String> {
        let file = FileSource::open(path)?;
        let src = al_bcdb::open(Box::new(file)).map_err(|e| format!("{}: {e}", path.display()))?;
        query::companies(&*src).map_err(|e| e.to_string())
    }

    fn find(&self, req: &TableRequest) -> Result<&BcTable, TableError> {
        let want = normalize_sql_name(&req.name);
        let mut hits: Vec<&BcTable> = self
            .tables
            .iter()
            .filter(|t| eq_ci(&t.table_name, &want))
            .filter(|t| match &t.company {
                Some(c) => req.per_company && eq_ci(c, &self.sql_company),
                None => !req.per_company,
            })
            .collect();
        if hits.len() > 1 {
            // Two apps may declare a table of one name; the id of the app the compiled table
            // belongs to is the only way to tell them apart.
            let by_app: Vec<&BcTable> = hits
                .iter()
                .copied()
                .filter(|t| match (&t.app_id, &req.app_id) {
                    (Some(a), Some(b)) => eq_ci(a, b),
                    _ => false,
                })
                .collect();
            if by_app.len() != 1 {
                let names: Vec<&str> = hits.iter().map(|t| t.sql_name()).collect();
                return Err(TableError::Ambiguous(format!(
                    "{} tables answer to '{}' ({}) and the declaring app's id selects {}",
                    hits.len(),
                    req.name,
                    names.join(" | "),
                    by_app.len()
                )));
            }
            hits = by_app;
        }
        hits.first().copied().ok_or(TableError::Absent)
    }
}

/// Which company a database restores.
fn pick_company(companies: &[String], asked: Option<&str>, file: &str) -> Result<String, String> {
    let list = || {
        companies
            .iter()
            .map(|c| format!("'{c}'"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(name) = asked {
        let n = normalize_sql_name(name);
        if let Some(c) = companies.iter().find(|c| **c == n || **c == name) {
            return Ok(c.clone());
        }
        let ci: Vec<&String> = companies.iter().filter(|c| eq_ci(c, &n)).collect();
        if let [c] = ci.as_slice() {
            return Ok((*c).clone());
        }
        let prefix: Vec<&String> = companies
            .iter()
            .filter(|c| c.to_uppercase().starts_with(&n.to_uppercase()))
            .collect();
        return match prefix.as_slice() {
            [c] => Ok((*c).clone()),
            [] => Err(format!(
                "--test-data-company '{name}' matches no company in {file}, which holds {}",
                list()
            )),
            many => Err(format!(
                "--test-data-company '{name}' matches {} companies in {file} ({}); use a longer prefix",
                many.len(),
                many.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(", ")
            )),
        };
    }
    match companies {
        [] => Err(format!("{file} holds no company, so there is no test data in it")),
        [only] => Ok(only.clone()),
        many => {
            let demo = normalize_sql_name(DEFAULT_COMPANY);
            many.iter().find(|c| eq_ci(c, &demo)).cloned().ok_or_else(|| {
                format!(
                    "{file} holds {} companies ({}) and none is '{DEFAULT_COMPANY}'; pick one with --test-data-company",
                    many.len(),
                    list()
                )
            })
        }
    }
}

/// The company's name as AL spells it: the `Name` of its row in the database's `Company` table
/// (SQL replaced the dot of `Ltd.`). A database without that table gives the SQL spelling.
fn al_company_name(src: &dyn BcSource, tables: &[BcTable], sql_company: &str) -> String {
    let found = (|| {
        let t = tables
            .iter()
            .find(|t| t.company.is_none() && t.app_id.is_none() && eq_ci(&t.table_name, "Company"))?;
        let cols = src.columns(&t.table).ok()?;
        let name = cols.iter().find(|c| eq_ci(&c.name, "Name"))?.clone();
        let mut hit = None;
        src.read_rows(&t.table, &[name], &mut |row| {
            if let Some(Value::Text(n)) = row.first() {
                if eq_ci(&normalize_sql_name(n), sql_company) {
                    hit = Some(n.clone());
                    return Ok(false);
                }
            }
            Ok(true)
        })
        .ok()?;
        hit
    })();
    found.unwrap_or_else(|| sql_company.to_string())
}

impl TableProvider for BackupProvider {
    fn company(&self) -> &str {
        &self.company
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn read(&self, req: &TableRequest) -> Result<Arc<TableRows>, TableError> {
        let table = self.find(req)?;
        let reader = |e: al_bcdb::Error| TableError::Reader(e.to_string());
        let plan = plan_read_table(
            &*self.src,
            None,
            table,
            &ReadRequest {
                merge_extensions: true,
                ..Default::default()
            },
        )
        .map_err(reader)?;
        let mut rows = Vec::new();
        plan.run(&*self.src, &mut |row| {
            rows.push(row.iter().map(cell).collect());
            Ok(true)
        })
        .map_err(reader)?;
        Ok(Arc::new(TableRows {
            columns: plan.headers.clone(),
            rows,
        }))
    }
}

/// The demo backup of the cached Microsoft artifact for a BC build: the folder under
/// `~/.bcartifacts.cache/sandbox` named like the first of `builds` that has one, else the newest
/// whose name starts with `major.minor`, else the newest of the major; then its `w1` (else `us`,
/// else the only) country folder. `builds` are the versions the run knows of the Microsoft apps it
/// loaded, so an artifact of another build than the apps' is never picked while theirs exists.
pub fn default_backup(builds: &[String], major: u16, minor: Option<u16>) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("HOME is not set")?;
    default_backup_in(&home.join(".bcartifacts.cache").join("sandbox"), builds, major, minor)
}

/// [`default_backup`] in the artifact cache `root`.
pub fn default_backup_in(root: &Path, builds: &[String], major: u16, minor: Option<u16>) -> Result<PathBuf, String> {
    let mut versions: Vec<String> = std::fs::read_dir(root)
        .map_err(|e| {
            format!(
                "no cached BC artifact in {} ({e}); name a backup: --test-data PATH.bak",
                root.display()
            )
        })?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    versions.sort_by_key(|v| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>());
    let exact = builds.iter().find_map(|b| versions.iter().find(|v| *v == b));
    let by_minor = || {
        let mi = minor?;
        versions.iter().rev().find(|v| v.starts_with(&format!("{major}.{mi}.")))
    };
    let by_major = || versions.iter().rev().find(|v| v.starts_with(&format!("{major}.")));
    let version = exact.or_else(by_minor).or_else(by_major).ok_or_else(|| {
        format!(
            "no cached BC artifact for BC {major} in {} (found: {}); name a backup: --test-data PATH.bak",
            root.display(),
            if versions.is_empty() {
                "none".to_string()
            } else {
                versions.join(", ")
            }
        )
    })?;
    let dir = root.join(version);
    let country = ["w1", "us"]
        .iter()
        .map(|c| dir.join(c))
        .find(|p| p.is_dir())
        .or_else(|| {
            std::fs::read_dir(&dir)
                .ok()?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "platform"))
        })
        .ok_or_else(|| format!("{} holds no country folder", dir.display()))?;
    let name = country
        .file_name()
        .map(|n| n.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    let bak = country.join(format!("BusinessCentral-{name}.bak"));
    if bak.is_file() {
        Ok(bak)
    } else {
        Err(format!("{} has no BusinessCentral-{name}.bak", country.display()))
    }
}
