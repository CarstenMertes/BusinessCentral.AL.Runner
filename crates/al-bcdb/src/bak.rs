//! The SQL Server backup path (filled in with the `.bak` phase).

use crate::error::{Error, Result};
use crate::io::ByteSource;
use crate::source::{BcSource, SourceTable};
use crate::types::SysColumn;
use crate::value::Value;

pub struct BakSource;

impl BakSource {
    pub fn open(_src: Box<dyn ByteSource>) -> Result<BakSource> {
        Err(Error::NotSupported("not implemented".into()))
    }
}

impl BcSource for BakSource {
    fn tables(&self) -> Result<&[SourceTable]> {
        unimplemented!()
    }
    fn row_count(&self, _t: &SourceTable) -> Result<i64> {
        unimplemented!()
    }
    fn columns(&self, _t: &SourceTable) -> Result<Vec<SysColumn>> {
        unimplemented!()
    }
    fn row_key_columns(&self, _t: &SourceTable) -> Result<Vec<String>> {
        unimplemented!()
    }
    fn read_rows(
        &self,
        _t: &SourceTable,
        _columns: &[SysColumn],
        _sink: &mut dyn FnMut(&[Value]) -> Result<bool>,
    ) -> Result<()> {
        unimplemented!()
    }
    fn banner(&self) -> Result<String> {
        unimplemented!()
    }
}
