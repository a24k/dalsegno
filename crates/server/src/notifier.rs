//! Draining the notification outbox (D-25).
//!
//! The row is marked after the send, so a crash between them repeats a message and never loses
//! one. Without a webhook, messages wait in the table.

use crate::runner::now_ms;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_app::notify::render;
use dalsegno_io::webhook::Webhook;
use dalsegno_store::notifications::Notification;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;

/// How often the outbox is looked at.
const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// The longest wait after failures.
const MAX_WAIT: Duration = Duration::from_secs(1800);
const BATCH: u32 = 20;

/// What sending gives back, with a message safe to log.
pub type SendFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Delivers a message.
pub trait Sender: Debug + Send + Sync + 'static {
    /// Sends a message.
    fn send(&self, text: String) -> SendFuture;
}

impl Sender for Webhook {
    fn send(&self, text: String) -> SendFuture {
        let hook = self.clone();
        Box::pin(async move {
            Webhook::send(&hook, &text)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

/// Sends what the outbox holds.
#[derive(Debug)]
pub struct Notifier {
    database: SharedDatabase,
    sender: Arc<dyn Sender>,
    public_url: Option<String>,
    poll: Duration,
}

impl Notifier {
    /// Makes a notifier.
    pub fn new(
        database: SharedDatabase,
        sender: Arc<dyn Sender>,
        public_url: Option<String>,
    ) -> Self {
        Self {
            database,
            sender,
            public_url,
            poll: POLL_INTERVAL,
        }
    }

    /// Runs until `shutdown` turns true. Failures make it wait longer, up to a limit.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut failures: u32 = 0;
        while !*shutdown.borrow() {
            match self.drain().await {
                Ok(()) => failures = 0,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(error, failures, "notification not sent");
                }
            }
            let wait = self
                .poll
                .saturating_mul(1 << failures.min(16))
                .min(MAX_WAIT.max(self.poll));
            tokio::select! {
                () = sleep(wait) => {}
                _ = shutdown.changed() => {}
            }
        }
    }

    /// Sends in order, and stops at the first failure so that order holds.
    async fn drain(&self) -> Result<(), String> {
        for notification in self.pending()? {
            let text = render(&notification, self.public_url.as_deref(), now_ms());
            let sent = self.sender.send(text).await;
            let database = lock(&self.database).map_err(|error| error.to_string())?;
            match sent {
                Ok(()) => database.mark_notification_sent(notification.id, now_ms()),
                Err(error) => {
                    database
                        .note_notification_attempt(notification.id)
                        .map_err(|stored| stored.to_string())?;
                    return Err(error);
                }
            }
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn pending(&self) -> Result<Vec<Notification>, String> {
        let database = lock(&self.database).map_err(|error| error.to_string())?;
        database
            .pending_notifications(BATCH)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::database::Database;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Recording {
        sent: Mutex<Vec<String>>,
        failures: Mutex<u32>,
    }

    impl Sender for Arc<Recording> {
        fn send(&self, text: String) -> SendFuture {
            let this = Arc::clone(self);
            Box::pin(async move {
                let mut failures = this.failures.lock().unwrap();
                if *failures > 0 {
                    *failures -= 1;
                    return Err("offline".to_string());
                }
                this.sent.lock().unwrap().push(text);
                Ok(())
            })
        }
    }

    fn database(subjects: &[&str]) -> SharedDatabase {
        let database = Database::open_in_memory().unwrap();
        for subject in subjects {
            database
                .enqueue_notification("other", subject, "{}", 0)
                .unwrap();
        }
        Arc::new(Mutex::new(database))
    }

    fn notifier(database: &SharedDatabase, sender: &Arc<Recording>) -> Notifier {
        Notifier {
            database: Arc::clone(database),
            sender: Arc::new(Arc::clone(sender)),
            public_url: None,
            poll: Duration::from_millis(2),
        }
    }

    fn pending(database: &SharedDatabase) -> usize {
        lock(database)
            .unwrap()
            .pending_notifications(100)
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn everything_waiting_goes_out_in_order() {
        let database = database(&["a", "b"]);
        let sender = Arc::new(Recording::default());

        notifier(&database, &sender).drain().await.unwrap();

        assert_eq!(pending(&database), 0);
        let sent = sender.sent.lock().unwrap();
        assert!(
            sent[0].ends_with(": a") && sent[1].ends_with(": b"),
            "{sent:?}"
        );
    }

    #[tokio::test]
    async fn a_failure_stops_the_batch_and_keeps_the_rest() {
        let database = database(&["a", "b"]);
        let sender = Arc::new(Recording::default());
        *sender.failures.lock().unwrap() = 1;

        assert!(notifier(&database, &sender).drain().await.is_err());

        assert_eq!(pending(&database), 2);
        assert!(sender.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_loop_recovers_after_a_failure() {
        let database = database(&["a"]);
        let sender = Arc::new(Recording::default());
        *sender.failures.lock().unwrap() = 2;
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(notifier(&database, &sender).run(shutdown));

        for _ in 0..500 {
            if pending(&database) == 0 {
                break;
            }
            sleep(Duration::from_millis(2)).await;
        }
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(pending(&database), 0);
    }
}
