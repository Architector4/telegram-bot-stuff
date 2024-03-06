mod list_watcher;

use std::{
    str::FromStr,
    sync::{atomic::AtomicBool, Arc},
};

pub use sqlx::Error;
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
    Executor, Row, Sqlite,
};
use teloxide::Bot;
use tokio::sync::watch;
use url::Url;

use super::types::{Domain, IsSpam};

type Pool = sqlx::Pool<Sqlite>;
const DB_PATH: &str = "sqlite:spam_domains.sqlite";
static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

pub struct Database {
    pool: Pool,
    drop_watch: (watch::Sender<()>, watch::Receiver<()>),
    bot: Bot,
}

impl Database {
    pub async fn new(bot: Bot) -> Result<Arc<Database>, Error> {
        assert!(
            !WAS_CONSTRUCTED.swap(true, std::sync::atomic::Ordering::SeqCst),
            "Second database was constructed. This is not allowed."
        );

        if !Sqlite::database_exists(DB_PATH).await.unwrap_or(false) {
            Sqlite::create_database(DB_PATH).await?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(32)
            .connect_with(
                SqliteConnectOptions::from_str(DB_PATH)
                    .unwrap()
                    .pragma("cache_size", "-32768")
                    .busy_timeout(std::time::Duration::from_secs(600)),
            )
            .await?;

        // Do some init. Create the tables...

        // DOMAINS:
        // domain (unique primary key, string)
        // example_url (string)
        // is_spam (0 for no, 1 for yes, 2 for unknown and needs review)
        // last_sent_to_review (date+time in UTC)
        // manually_reviewed (0 for no, 1 for yes)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS domains (
                    domain TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    example_url TEXT NULL,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL,
                    manually_reviewed INTEGER NOT NULL DEFAULT 0
                ) STRICT;",
        ))
        .await?;

        // URLS:
        // url (unique primary key, string)
        // is_spam (0 for no, 1 for yes, 2 for unknown and needs review)
        // last_sent_to_review (date+time in UTC)
        // manually_reviewed (0 for no, 1 for yes)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS urls (
                    url TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL,
                    manually_reviewed INTEGER NOT NULL DEFAULT 0
                ) STRICT;",
        ))
        .await?;

        // Transparent database migration lololol
        // Will fail harmlessly if the column already exists.
        let _ = sqlx::query(
            "ALTER TABLE domains
        ADD COLUMN manually_reviewed INTEGER NOT NULL DEFAULT 0;",
        )
        .execute(&pool)
        .await;

        let db_arc = Arc::new(Database {
            pool,
            bot,
            drop_watch: watch::channel(()),
        });

        // Spawn the watcher.
        tokio::spawn(list_watcher::watch_list(db_arc.clone()));

        Ok(db_arc)
    }

    /// Check if a domain is a spam domain or not, according to the database.
    /// Returns [`None`] if it's not in the database.
    ///
    /// Note that [`Self::is_url_spam`] should take priority over this,
    /// unless its return result is [`IsSpam::Maybe`].
    pub async fn is_domain_spam(&self, domain: &Domain) -> Result<Option<IsSpam>, Error> {
        sqlx::query("SELECT is_spam FROM domains WHERE domain=?;")
            .bind(domain.as_str())
            .map(|row: SqliteRow| IsSpam::from(row.get::<u8, _>("is_spam")))
            .fetch_optional(&self.pool)
            .await
    }

    /// Check if a URL is a spam URL or not, according to the database.
    /// Returns [`None`] if it's not in the database.
    ///
    /// Note that this should take priority over [`Self::is_domain_spam`],
    /// unless this function's return result is [`IsSpam::Maybe`].
    pub async fn is_url_spam(&self, url: &Url) -> Result<Option<IsSpam>, Error> {
        sqlx::query("SELECT is_spam FROM urls WHERE url=?;")
            .bind(url.as_str())
            .map(|row: SqliteRow| IsSpam::from(row.get::<u8, _>("is_spam")))
            .fetch_optional(&self.pool)
            .await
    }

    /// Check if a given URL (or its domain) is spam or not, according to the database.
    /// Convenience method for [`Self::is_domain_spam`] and [`Self::is_url_spam`]
    /// Returns [`None`] if it's not in the database.
    ///
    /// Argument `domain` is optional and, if `url` check is indecisive,
    /// is used if provided, or extracted from URL if not.
    pub async fn is_spam(
        &self,
        url: &Url,
        mut domain: Option<&Domain>,
    ) -> Result<Option<IsSpam>, Error> {
        let mut url_maybe_spam = false;
        // Look for direct URL match...
        if let Some(url_result) = self.is_url_spam(url).await? {
            if url_result == IsSpam::Maybe {
                url_maybe_spam = true;
            } else {
                return Ok(Some(url_result));
            }
        }

        // If no provided domain, try to get one from the URL.
        // Otherwise, use provided domain, to not do an extraneous allocation.
        let domain_inner;
        if domain.is_none() {
            domain_inner = Domain::from_url(url);
            domain = domain_inner.as_ref();
        }

        // Look for domain match...
        if let Some(domain) = domain {
            self.is_domain_spam(domain).await
        } else {
            // If it's not a domain, but URL is marked as "maybe", return "maybe".
            match url_maybe_spam {
                true => Ok(Some(IsSpam::Maybe)),
                false => Ok(None),
            }
        }
    }

    /// Inserts a domain into the database and tags it as spam or not.
    /// Overwrites the domain if it already exists.
    pub async fn add_domain(
        &self,
        domain: &Domain,
        example_url: Option<&Url>,
        is_spam: IsSpam,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO domains(domain, example_url, is_spam)
            VALUES (?, ?, ?)
        ON CONFLICT DO
            UPDATE SET example_url=COALESCE(?, example_url), is_spam=?;",
        )
        .bind(domain.as_str())
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a domain as maybe spam, if it's not already marked as spam
    /// and wasn't manually reviewed.
    pub async fn mark_domain_sus(
        &self,
        domain: &Domain,
        example_url: Option<&Url>,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO domains(domain, example_url, is_spam)
            VALUES (?, ?, 2)
        ON CONFLICT DO
            UPDATE SET example_url=COALESCE(?, example_url), is_spam=2
        WHERE is_spam=0 AND manually_reviewed=0;",
        )
        .bind(domain.as_str())
        .bind(example_url.map(Url::as_str))
        .bind(example_url.map(Url::as_str))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Inserts a URL into the database and tags it as spam or not.
    /// Overwrites the URL if it already exists.
    pub async fn add_url(&self, url: &Url, is_spam: IsSpam) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO urls(url, is_spam)
            VALUES (?, ?)
        ON CONFLICT DO
            UPDATE SET is_spam=?;",
        )
        .bind(url.as_str())
        .bind::<u8>(is_spam.into())
        .bind::<u8>(is_spam.into())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a URL as maybe spam, if it's not already marked as spam
    /// and wasn't manually reviewed.
    pub async fn mark_url_sus(&self, url: &Url) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO urls(url, is_spam)
            VALUES (?, 2)
        ON CONFLICT DO
            UPDATE SET is_spam=2
        WHERE is_spam=0 AND manually_reviewed=0;",
        )
        .bind(url.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Convenience function to mark both a URL and its domain as maybe spam.
    pub async fn mark_sus(&self, url: &Url, mut domain: Option<&Domain>) -> Result<(), Error> {
        self.mark_url_sus(url).await?;

        // If no provided domain, try to get one from the URL.
        // Otherwise, use provided domain, to not do an extraneous allocation.
        let domain_inner;
        if domain.is_none() {
            domain_inner = Domain::from_url(url);
            domain = domain_inner.as_ref();
        }
        if let Some(domain) = domain {
            self.mark_domain_sus(domain, Some(url)).await?;
        }

        Ok(())
    }
}
