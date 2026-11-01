//! The notification outbox, which a dispatcher drains (D-25).

use crate::database::{Database, StoreError};
use rusqlite::params;

/// A notification waiting to be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Row ID.
    pub id: i64,
    /// What it is about.
    pub kind: String,
    /// What makes it unique, such as `change:12:confirmed`.
    pub subject: String,
    /// JSON.
    pub body: String,
}

/// A change whose DVR deadline is near.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineChange {
    /// Row ID.
    pub id: i64,
    /// The deadline.
    pub deadline_at: i64,
    /// Title of the stream.
    pub title: String,
}

impl Database {
    /// Returns the oldest notifications not sent yet.
    pub fn pending_notifications(&self, limit: u32) -> Result<Vec<Notification>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, kind, subject, body FROM notifications \
             WHERE sent_at IS NULL ORDER BY id LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            Ok(Notification {
                id: row.get(0)?,
                kind: row.get(1)?,
                subject: row.get(2)?,
                body: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records that a notification went out.
    pub fn mark_notification_sent(&self, id: i64, now: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE notifications SET sent_at = ?2, attempts = attempts + 1 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    /// Records a failed attempt.
    pub fn note_notification_attempt(&self, id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE notifications SET attempts = attempts + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Returns open changes whose deadline falls before `before`.
    pub fn changes_near_deadline(&self, before: i64) -> Result<Vec<DeadlineChange>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT changes.id, dvr_deadline_at, streams.title FROM changes \
             JOIN streams ON streams.id = changes.stream_id \
             WHERE changes.state IN ('provisional', 'confirmed') AND dvr_deadline_at <= ?1 \
             ORDER BY changes.id",
        )?;
        let rows = statement.query_map([before], |row| {
            Ok(DeadlineChange {
                id: row.get(0)?,
                deadline_at: row.get(1)?,
                title: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_one() -> Database {
        let database = Database::open_in_memory().unwrap();
        database
            .enqueue_notification("change", "change:1:confirmed", "{}", 0)
            .unwrap();
        database
    }

    #[test]
    fn a_new_notification_is_pending() {
        let pending = with_one().pending_notifications(10).unwrap();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].subject, "change:1:confirmed");
    }

    #[test]
    fn a_sent_notification_is_not_pending() {
        let database = with_one();
        let id = database.pending_notifications(10).unwrap()[0].id;

        database.mark_notification_sent(id, 5).unwrap();

        assert_eq!(database.pending_notifications(10).unwrap(), []);
    }

    #[test]
    fn a_failed_attempt_stays_pending() {
        let database = with_one();
        let id = database.pending_notifications(10).unwrap()[0].id;

        database.note_notification_attempt(id).unwrap();

        assert_eq!(database.pending_notifications(10).unwrap().len(), 1);
    }

    #[test]
    fn the_oldest_go_first() {
        let database = with_one();
        database
            .enqueue_notification("stream", "b", "{}", 1)
            .unwrap();

        let pending = database.pending_notifications(1).unwrap();

        assert_eq!(pending[0].subject, "change:1:confirmed");
    }
}
