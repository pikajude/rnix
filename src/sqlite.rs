use crate::prelude::*;
use rusqlite::{Connection, DatabaseName};

#[derive(Deref, DerefMut, Debug)]
pub struct Sqlite(Connection);

impl Sqlite {
  pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
    let mut conn = Connection::open(path)?;
    if slog_scope::logger().is_trace_enabled() {
      conn.trace(Some(|x| trace!("{}", x)));
    }
    Ok(Self(conn))
  }

  pub fn _set_is_cache(&self) -> Result<()> {
    self.pragma_update(None, "synchronous", &"off")?;
    self.pragma_update(Some(DatabaseName::Main), "journal_mode", &"truncate")?;
    Ok(())
  }
}
