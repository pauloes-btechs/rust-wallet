use crate::PaymentIntent;
use std::collections::BTreeMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("intent store I/O: {0}")]
    Io(String),
    #[error("intent store data is corrupt: {0}")]
    Corrupt(String),
}

/// Durable storage for intents. `save` must be durable when it returns:
/// the engine relies on that to order "persist" strictly before "dispatch".
pub trait IntentStore: Send + Sync {
    fn save(&self, intent: &PaymentIntent) -> Result<(), StoreError>;
    fn get(&self, intent_id: &str) -> Result<Option<PaymentIntent>, StoreError>;
    /// All intents, oldest first.
    fn list(&self) -> Result<Vec<PaymentIntent>, StoreError>;
}

/// Insertion sequence + intents keyed by id.
type Seqd = (u64, BTreeMap<String, (u64, PaymentIntent)>);

/// In-memory store for tests and the web demo.
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Seqd>,
}

impl IntentStore for MemoryStore {
    fn save(&self, intent: &PaymentIntent) -> Result<(), StoreError> {
        let mut g = self.inner.lock().map_err(|e| StoreError::Io(e.to_string()))?;
        let seq = match g.1.get(&intent.intent_id) {
            Some((seq, _)) => *seq,
            None => {
                g.0 += 1;
                g.0
            }
        };
        g.1.insert(intent.intent_id.clone(), (seq, intent.clone()));
        Ok(())
    }

    fn get(&self, intent_id: &str) -> Result<Option<PaymentIntent>, StoreError> {
        let g = self.inner.lock().map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(g.1.get(intent_id).map(|(_, i)| i.clone()))
    }

    fn list(&self) -> Result<Vec<PaymentIntent>, StoreError> {
        let g = self.inner.lock().map_err(|e| StoreError::Io(e.to_string()))?;
        let mut v: Vec<_> = g.1.values().cloned().collect();
        v.sort_by_key(|(seq, _)| *seq);
        Ok(v.into_iter().map(|(_, i)| i).collect())
    }
}

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

#[cfg(feature = "sqlite")]
mod sqlite {
    use super::*;
    use rusqlite::{params, Connection, OptionalExtension};
    use std::path::Path;

    /// SQLite store (WAL, `synchronous=FULL`): a save is on disk before it returns.
    pub struct SqliteStore {
        conn: Mutex<Connection>,
    }

    fn io(e: impl std::fmt::Display) -> StoreError {
        StoreError::Io(e.to_string())
    }

    impl SqliteStore {
        pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
            Self::init(Connection::open(path).map_err(io)?)
        }

        pub fn in_memory() -> Result<Self, StoreError> {
            Self::init(Connection::open_in_memory().map_err(io)?)
        }

        fn init(conn: Connection) -> Result<Self, StoreError> {
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=FULL;
                 CREATE TABLE IF NOT EXISTS payment_intents (
                   seq        INTEGER PRIMARY KEY AUTOINCREMENT,
                   intent_id  TEXT NOT NULL UNIQUE,
                   state      TEXT NOT NULL,
                   body       TEXT NOT NULL
                 );",
            )
            .map_err(io)?;
            Ok(Self { conn: Mutex::new(conn) })
        }
    }

    fn decode(body: String) -> Result<PaymentIntent, StoreError> {
        serde_json::from_str(&body).map_err(|e| StoreError::Corrupt(e.to_string()))
    }

    impl IntentStore for SqliteStore {
        fn save(&self, intent: &PaymentIntent) -> Result<(), StoreError> {
            let body = serde_json::to_string(intent).map_err(io)?;
            let state = serde_json::to_value(intent.state).map_err(io)?;
            let conn = self.conn.lock().map_err(io)?;
            conn.execute(
                "INSERT INTO payment_intents (intent_id, state, body) VALUES (?1, ?2, ?3)
                 ON CONFLICT(intent_id) DO UPDATE SET state = excluded.state, body = excluded.body",
                params![intent.intent_id, state.as_str().unwrap_or_default(), body],
            )
            .map_err(io)?;
            Ok(())
        }

        fn get(&self, intent_id: &str) -> Result<Option<PaymentIntent>, StoreError> {
            let conn = self.conn.lock().map_err(io)?;
            conn.query_row("SELECT body FROM payment_intents WHERE intent_id = ?1", [intent_id], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(io)?
            .map(decode)
            .transpose()
        }

        fn list(&self) -> Result<Vec<PaymentIntent>, StoreError> {
            let conn = self.conn.lock().map_err(io)?;
            let mut stmt = conn.prepare("SELECT body FROM payment_intents ORDER BY seq").map_err(io)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(io)?;
            rows.map(|r| r.map_err(io).and_then(decode)).collect()
        }
    }
}

impl<T: IntentStore + ?Sized> IntentStore for std::sync::Arc<T> {
    fn save(&self, intent: &PaymentIntent) -> Result<(), StoreError> {
        (**self).save(intent)
    }
    fn get(&self, intent_id: &str) -> Result<Option<PaymentIntent>, StoreError> {
        (**self).get(intent_id)
    }
    fn list(&self) -> Result<Vec<PaymentIntent>, StoreError> {
        (**self).list()
    }
}
