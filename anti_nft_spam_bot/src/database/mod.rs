mod list_watcher;

use std::{
    str::FromStr,
    sync::{atomic::AtomicBool, Arc},
};

use chrono::Utc;
pub use sqlx::Error;
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
    Executor, Row, Sqlite,
};
use teloxide::Bot;
use tokio::sync::{watch, Mutex};
use url::Url;

use crate::parse_url_like_telegram;

use super::types::{Domain, IsSpam};

type Pool = sqlx::Pool<Sqlite>;
const DB_PATH: &str = "sqlite:spam_domains.sqlite";
static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

pub struct Database {
    pool: Pool,
    drop_watch: (watch::Sender<()>, watch::Receiver<()>),
    bot: Bot,
    // Mutexes are bad. However, this will only be used for reviews,
    // which can only be done by a few people in the control chat.
    review_lock: Mutex<()>,
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
        // last_sent_to_review (date+time in UTC timezone in ISO 8601 format)
        // manually_reviewed (0 for no, 1 for yes)
        // from_spam_list (0 for no, 1 for yes)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS domains (
                    domain TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    example_url TEXT NULL,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL,
                    manually_reviewed INTEGER NOT NULL DEFAULT 0,
                    from_spam_list INTEGER NOT NULL DEFAULT 0
                ) STRICT;",
        ))
        .await?;

        // URLS:
        // url (unique primary key, string)
        // is_spam (0 for no, 1 for yes, 2 for unknown and needs review)
        // last_sent_to_review (date+time in UTC timezone in ISO 8601 format)
        // manually_reviewed (0 for no, 1 for yes)
        // from_spam_list (0 for no, 1 for yes)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS urls (
                    url TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL,
                    manually_reviewed INTEGER NOT NULL DEFAULT 0,
                    from_spam_list INTEGER NOT NULL DEFAULT 0
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
        let _ = sqlx::query(
            "ALTER TABLE domains
        ADD COLUMN from_spam_list INTEGER NOT NULL DEFAULT 0;",
        )
        .execute(&pool)
        .await;
        let _ = sqlx::query(
            "ALTER TABLE urls
        ADD COLUMN from_spam_list INTEGER NOT NULL DEFAULT 0;",
        )
        .execute(&pool)
        .await;

        let db_arc = Arc::new(Database {
            pool,
            bot,
            review_lock: Mutex::new(()),
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
        from_spam_list: bool,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO domains(domain, example_url, is_spam, from_spam_list)
            VALUES (?, ?, ?, ?)
        ON CONFLICT DO
            UPDATE SET example_url=COALESCE(?, example_url), is_spam=?, from_spam_list=?;",
        )
        .bind(domain.as_str())
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .execute(&self.pool)
        .await?;

        // If we know for a fact that this URL and its domain is
        // spam, we don't need an entry in the `urls` table for it.
        if let Some(url) = example_url {
            sqlx::query("DELETE FROM urls WHERE url=?;")
                .bind(url.as_str())
                .execute(&self.pool)
                .await?;
        }
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
    pub async fn add_url(
        &self,
        url: &Url,
        is_spam: IsSpam,
        from_spam_list: bool,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO urls(url, is_spam, from_spam_list)
            VALUES (?, ?, ?)
        ON CONFLICT DO
            UPDATE SET is_spam=?, from_spam_list=?;",
        )
        .bind(url.as_str())
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
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

    /// Delete all entries added from the spam list.
    pub async fn clean_all_from_spam_list(&self) -> Result<(), Error> {
        sqlx::query("DELETE FROM domains WHERE from_spam_list=1")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM urls WHERE from_spam_list=1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get a URL for review and its state in the database.
    pub async fn get_url_for_review(&self) -> Result<Option<(Url, IsSpam)>, Error> {
        // I don't like repeating myself much, so here goes this.
        // This should be used with a query that returns three values:
        // a URL, is_spam, and a "key" string.
        // The "key" is for writing back the time when this URL was sent to reviews.
        macro_rules! try_get_url {
            ($query:expr) => {
                sqlx::query($query)
                    .map(|row: SqliteRow| {
                        (
                            parse_url_like_telegram(row.get(0))
                                .expect("Database has invalid URL data!"),
                            IsSpam::from(row.get::<u8, _>("is_spam")),
                            row.get::<String, _>("key"),
                        )
                    })
                    .fetch_optional(&self.pool)
                    .await?
            };
        }

        // If this is true, the URL was gotten from the "urls" table; if false, "domains" table.
        let mut url_from_urls_table;

        // Get the mutex. It'll be unlocked at the end of the function
        // automatically due to RAII.
        let _the_mutex = self.review_lock.lock();

        // Check the URLs table for a "maybe spam" URL we can grab first.
        url_from_urls_table = true;
        let mut db_result: Option<(Url, IsSpam, String)> = try_get_url!(
            "SELECT url, url AS key, is_spam FROM urls
            WHERE is_spam=2 AND from_spam_list=0
            ORDER BY manually_reviewed, last_sent_to_review LIMIT 1;"
        );

        if db_result.is_none() {
            // Check the domains table for a "maybe spam" URL?
            url_from_urls_table = false;
            db_result = try_get_url!(
                "SELECT COALESCE(example_url, domain), domain AS key, is_spam
                FROM domains WHERE is_spam=2 AND from_spam_list=0
                ORDER BY manually_reviewed, last_sent_to_review LIMIT 1;"
            );

            if db_result.is_none() {
                // Rehash URLs that are marked as spam, just in case they are falsely marked,
                // and then URLs that are marked as not spam.
                // Check the URLs table...
                url_from_urls_table = true;
                db_result = try_get_url!(
                    "SELECT url, url AS key, is_spam
                    FROM urls WHERE is_spam IN (0,1) AND from_spam_list=0
                    ORDER BY manually_reviewed, last_sent_to_review, is_spam LIMIT 1;"
                );
                if db_result.is_none() {
                    // Check the domains table for rehashing...
                    url_from_urls_table = false;
                    db_result = try_get_url!(
                        "SELECT COALESCE(example_url, domain), domain AS key, is_spam
                        FROM domains WHERE is_spam IN (0,1) AND from_spam_list=0
                        ORDER BY manually_reviewed, last_sent_to_review, is_spam DESC LIMIT 1;"
                    );
                }
            }
        }

        let Some((url, is_spam, db_key)) = db_result else {
            // Well dang.
            return Ok(None);
        };

        let db_query = if url_from_urls_table {
            "UPDATE urls SET last_sent_to_review=? WHERE url=?;"
        } else {
            "UPDATE domains SET last_sent_to_review=? WHERE domain=?;"
        };

        // Mark this URL or domain in the database as sent to review.
        let time = Utc::now();
        sqlx::query(db_query)
            .bind(time)
            .bind(db_key.as_str())
            .execute(&self.pool)
            .await?;

        // Pass it on.
        Ok(Some((url, is_spam)))
    }
}
