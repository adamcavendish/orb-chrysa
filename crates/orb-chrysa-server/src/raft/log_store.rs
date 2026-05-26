#![allow(clippy::result_large_err)]

use std::fmt::Debug;
use std::ops::RangeBounds;
use std::path::Path;

use openraft::storage::LogFlushed;
use openraft::{Entry, LogId, StorageError, StorageIOError, Vote};
use redb::{ReadableDatabase, ReadableTable};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use super::TypeConfig;

#[derive(Debug, Error)]
pub enum LogStoreError {
    #[error("database error: {0}")]
    Database(#[from] redb::DatabaseError),
    #[error("failed to spawn actor thread: {0}")]
    SpawnThread(#[from] std::io::Error),
}

type LogEntry = Entry<TypeConfig>;

const CHANNEL_CAPACITY: usize = 4096;

enum Command {
    GetLogState {
        tx: oneshot::Sender<Result<LogState, StorageError<u64>>>,
    },
    GetEntries {
        start: u64,
        end: u64,
        tx: oneshot::Sender<Result<Vec<LogEntry>, StorageError<u64>>>,
    },
    Append {
        entries: Vec<LogEntry>,
        callback: LogFlushed<TypeConfig>,
    },
    Truncate {
        log_id: LogId<u64>,
        tx: oneshot::Sender<Result<(), StorageError<u64>>>,
    },
    Purge {
        log_id: LogId<u64>,
        tx: oneshot::Sender<Result<(), StorageError<u64>>>,
    },
    SaveVote {
        vote: Vote<u64>,
        tx: oneshot::Sender<Result<(), StorageError<u64>>>,
    },
    SeedVote {
        vote: Vote<u64>,
        tx: oneshot::Sender<Result<(), StorageError<u64>>>,
    },
    ReadVote {
        tx: oneshot::Sender<Result<Option<Vote<u64>>, StorageError<u64>>>,
    },
    Compact {
        tx: oneshot::Sender<Result<(), StorageError<u64>>>,
    },
}

struct LogState {
    last_purged_log_id: Option<LogId<u64>>,
    last_log_id: Option<LogId<u64>>,
}

const TABLE_LOGS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("logs");
const TABLE_META: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("meta");

const META_VOTE: &str = "vote";
const META_LAST_PURGED: &str = "last_purged";

fn run_actor(mut db: redb::Database, mut rx: mpsc::Receiver<Command>) {
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            Command::GetLogState { tx } => {
                let result = get_log_state_sync(&db);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::GetEntries { start, end, tx } => {
                let result = get_entries_sync(&db, start, end);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::Append { entries, callback } => {
                let result = append_sync(&db, &entries);
                match result {
                    Ok(()) => callback.log_io_completed(Ok(())),
                    Err(_) => {
                        callback.log_io_completed(Err(std::io::Error::other("redb write failed")));
                    }
                }
            }
            Command::Truncate { log_id, tx } => {
                let result = truncate_sync(&db, log_id);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::Purge { log_id, tx } => {
                let result = purge_sync(&db, log_id);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::SaveVote { vote, tx } => {
                let result = save_vote_sync(&db, &vote);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::SeedVote { vote, tx } => {
                let result = seed_vote_sync(&db, &vote);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::ReadVote { tx } => {
                let result = read_vote_sync(&db);
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
            Command::Compact { tx } => {
                let result = db
                    .compact()
                    .map(|_| ())
                    .map_err(|e| storage_io_err("compact", &e));
                if tx.send(result).is_err() {
                    tracing::warn!("log_store actor: receiver dropped");
                }
            }
        }
    }
}

#[derive(Debug)]
struct StoreErr(String);
impl std::fmt::Display for StoreErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StoreErr {}

fn bc_encode<T: serde::Serialize>(val: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(val)
}

fn bc_decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

fn storage_io_err(op: &str, e: &impl std::fmt::Display) -> StorageError<u64> {
    StorageError::IO {
        source: StorageIOError::new(
            openraft::ErrorSubject::Logs,
            openraft::ErrorVerb::Write,
            openraft::AnyError::new(&StoreErr(format!("{}: {}", op, e))),
        ),
    }
}

fn get_log_state_sync(db: &redb::Database) -> Result<LogState, StorageError<u64>> {
    let txn = db
        .begin_read()
        .map_err(|e| storage_io_err("begin_read", &e))?;

    let last_purged_log_id = match txn.open_table(TABLE_META) {
        Ok(table) => table
            .get(META_LAST_PURGED)
            .map_err(|e| storage_io_err("read_last_purged", &e))?
            .map(|v| bc_decode::<LogId<u64>>(v.value()))
            .transpose()
            .map_err(|e| storage_io_err("deserialize_last_purged", &e))?,
        Err(redb::TableError::TableDoesNotExist(_)) => None,
        Err(e) => return Err(storage_io_err("open_meta_table", &e)),
    };

    let last_log_id = match txn.open_table(TABLE_LOGS) {
        Ok(table) => table
            .last()
            .map_err(|e| storage_io_err("last_log", &e))?
            .map(|(_, v)| bc_decode::<LogEntry>(v.value()))
            .transpose()
            .map_err(|e| storage_io_err("deserialize_last_entry", &e))?
            .map(|e| e.log_id),
        Err(redb::TableError::TableDoesNotExist(_)) => None,
        Err(e) => return Err(storage_io_err("open_logs_table", &e)),
    };

    let last_log_id = last_log_id.or(last_purged_log_id);

    Ok(LogState {
        last_purged_log_id,
        last_log_id,
    })
}

fn get_entries_sync(
    db: &redb::Database,
    start: u64,
    end: u64,
) -> Result<Vec<LogEntry>, StorageError<u64>> {
    let txn = db
        .begin_read()
        .map_err(|e| storage_io_err("begin_read", &e))?;
    let table = match txn.open_table(TABLE_LOGS) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(e) => return Err(storage_io_err("open_logs_table", &e)),
    };

    let mut entries = Vec::new();
    let range = table
        .range(start..end)
        .map_err(|e| storage_io_err("range", &e))?;
    for item in range {
        let (_, v) = item.map_err(|e| storage_io_err("range_iter", &e))?;
        let entry: LogEntry =
            bc_decode(v.value()).map_err(|e| storage_io_err("deserialize_entry", &e))?;
        entries.push(entry);
    }

    Ok(entries)
}

fn append_sync(db: &redb::Database, entries: &[LogEntry]) -> Result<(), StorageError<u64>> {
    let txn = db
        .begin_write()
        .map_err(|e| storage_io_err("begin_write", &e))?;
    {
        let mut table = txn
            .open_table(TABLE_LOGS)
            .map_err(|e| storage_io_err("open_logs_table", &e))?;
        for entry in entries {
            let bytes = bc_encode(entry).map_err(|e| storage_io_err("serialize_entry", &e))?;
            table
                .insert(entry.log_id.index, bytes.as_slice())
                .map_err(|e| storage_io_err("insert_entry", &e))?;
        }
    }
    txn.commit()
        .map_err(|e| storage_io_err("commit_append", &e))?;
    Ok(())
}

fn truncate_sync(db: &redb::Database, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
    let txn = db
        .begin_write()
        .map_err(|e| storage_io_err("begin_write", &e))?;
    {
        let mut table = txn
            .open_table(TABLE_LOGS)
            .map_err(|e| storage_io_err("open_logs_table", &e))?;
        let keys_to_remove: Vec<u64> = table
            .range(log_id.index..)
            .map_err(|e| storage_io_err("range", &e))?
            .map(|item| item.map(|(k, _)| k.value()))
            .collect::<Result<_, _>>()
            .map_err(|e| storage_io_err("range_iter", &e))?;
        for key in keys_to_remove {
            table
                .remove(key)
                .map_err(|e| storage_io_err("remove_entry", &e))?;
        }
    }
    txn.commit()
        .map_err(|e| storage_io_err("commit_truncate", &e))?;
    Ok(())
}

fn purge_sync(db: &redb::Database, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
    let txn = db
        .begin_write()
        .map_err(|e| storage_io_err("begin_write", &e))?;
    {
        let mut table = txn
            .open_table(TABLE_LOGS)
            .map_err(|e| storage_io_err("open_logs_table", &e))?;
        let keys_to_remove: Vec<u64> = table
            .range(..=log_id.index)
            .map_err(|e| storage_io_err("range", &e))?
            .map(|item| item.map(|(k, _)| k.value()))
            .collect::<Result<_, _>>()
            .map_err(|e| storage_io_err("range_iter", &e))?;
        for key in keys_to_remove {
            table
                .remove(key)
                .map_err(|e| storage_io_err("remove_entry", &e))?;
        }

        let mut meta = txn
            .open_table(TABLE_META)
            .map_err(|e| storage_io_err("open_meta_table", &e))?;
        let bytes = bc_encode(&log_id).map_err(|e| storage_io_err("serialize_purged", &e))?;
        meta.insert(META_LAST_PURGED, bytes.as_slice())
            .map_err(|e| storage_io_err("insert_purged", &e))?;
    }
    txn.commit()
        .map_err(|e| storage_io_err("commit_purge", &e))?;
    Ok(())
}

fn save_vote_sync(db: &redb::Database, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
    let txn = db
        .begin_write()
        .map_err(|e| storage_io_err("begin_write", &e))?;
    {
        let mut table = txn
            .open_table(TABLE_META)
            .map_err(|e| storage_io_err("open_meta_table", &e))?;
        let bytes = bc_encode(vote).map_err(|e| storage_io_err("serialize_vote", &e))?;
        table
            .insert(META_VOTE, bytes.as_slice())
            .map_err(|e| storage_io_err("insert_vote", &e))?;
    }
    txn.commit()
        .map_err(|e| storage_io_err("commit_vote", &e))?;
    Ok(())
}

fn seed_vote_sync(db: &redb::Database, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
    let current = read_vote_sync(db)?;
    if current.as_ref().is_some_and(|current| current >= vote) {
        return Ok(());
    }
    save_vote_sync(db, vote)
}

fn read_vote_sync(db: &redb::Database) -> Result<Option<Vote<u64>>, StorageError<u64>> {
    let txn = db
        .begin_read()
        .map_err(|e| storage_io_err("begin_read", &e))?;
    let table = match txn.open_table(TABLE_META) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(storage_io_err("open_meta_table", &e)),
    };
    let vote = table
        .get(META_VOTE)
        .map_err(|e| storage_io_err("read_vote", &e))?
        .map(|v| bc_decode::<Vote<u64>>(v.value()))
        .transpose()
        .map_err(|e| storage_io_err("deserialize_vote", &e))?;
    Ok(vote)
}

// ── Public API ──

pub struct RedbLogStore {
    tx: mpsc::Sender<Command>,
}

impl RedbLogStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, LogStoreError> {
        let db = redb::Database::create(path.as_ref())?;

        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);

        std::thread::Builder::new()
            .name("raft-log-actor".into())
            .spawn(move || run_actor(db, rx))?;

        Ok(Self { tx })
    }

    pub async fn seed_vote(&self, vote: Vote<u64>) -> Result<(), StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::SeedVote { vote, tx })
            .await
            .map_err(|_| storage_io_err("send_seed_vote", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_seed_vote", &"channel closed"))?
    }

    #[expect(
        dead_code,
        reason = "manual redb compaction hook is reserved for snapshot policy"
    )]
    pub async fn compact(&self) -> Result<(), StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Compact { tx })
            .await
            .map_err(|_| storage_io_err("send_compact", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_compact", &"channel closed"))?
    }
}

impl openraft::storage::RaftLogReader<TypeConfig> for RedbLogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<LogEntry>, StorageError<u64>> {
        let start = match range.start_bound() {
            std::ops::Bound::Included(&v) => v,
            std::ops::Bound::Excluded(&v) => v + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&v) => v + 1,
            std::ops::Bound::Excluded(&v) => v,
            std::ops::Bound::Unbounded => u64::MAX,
        };

        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::GetEntries { start, end, tx })
            .await
            .map_err(|_| storage_io_err("send_get_entries", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_get_entries", &"channel closed"))?
    }
}

impl openraft::storage::RaftLogStorage<TypeConfig> for RedbLogStore {
    type LogReader = RedbLogReader;

    async fn get_log_state(
        &mut self,
    ) -> Result<openraft::storage::LogState<TypeConfig>, StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::GetLogState { tx })
            .await
            .map_err(|_| storage_io_err("send_get_log_state", &"channel closed"))?;
        let state = rx
            .await
            .map_err(|_| storage_io_err("recv_get_log_state", &"channel closed"))??;
        Ok(openraft::storage::LogState {
            last_purged_log_id: state.last_purged_log_id,
            last_log_id: state.last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        RedbLogReader {
            tx: self.tx.clone(),
        }
    }

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::SaveVote { vote: *vote, tx })
            .await
            .map_err(|_| storage_io_err("send_save_vote", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_save_vote", &"channel closed"))?
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::ReadVote { tx })
            .await
            .map_err(|_| storage_io_err("send_read_vote", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_read_vote", &"channel closed"))?
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = LogEntry> + Send,
        I::IntoIter: Send,
    {
        let entries: Vec<LogEntry> = entries.into_iter().collect();
        self.tx
            .send(Command::Append { entries, callback })
            .await
            .map_err(|_| storage_io_err("send_append", &"channel closed"))?;
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Truncate { log_id, tx })
            .await
            .map_err(|_| storage_io_err("send_truncate", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_truncate", &"channel closed"))?
    }

    async fn purge(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Purge { log_id, tx })
            .await
            .map_err(|_| storage_io_err("send_purge", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_purge", &"channel closed"))?
    }
}

pub struct RedbLogReader {
    tx: mpsc::Sender<Command>,
}

impl openraft::storage::RaftLogReader<TypeConfig> for RedbLogReader {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<LogEntry>, StorageError<u64>> {
        let start = match range.start_bound() {
            std::ops::Bound::Included(&v) => v,
            std::ops::Bound::Excluded(&v) => v + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&v) => v + 1,
            std::ops::Bound::Excluded(&v) => v,
            std::ops::Bound::Unbounded => u64::MAX,
        };

        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::GetEntries { start, end, tx })
            .await
            .map_err(|_| storage_io_err("send_get_entries", &"channel closed"))?;
        rx.await
            .map_err(|_| storage_io_err("recv_get_entries", &"channel closed"))?
    }
}
