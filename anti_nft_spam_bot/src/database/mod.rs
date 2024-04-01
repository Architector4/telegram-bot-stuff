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

use crate::{parse_url_like_telegram, types::ReviewResponse};

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
        manually_reviewed: bool,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO domains(domain, example_url, is_spam, from_spam_list, manually_reviewed)
            VALUES (?, ?, ?, ?, ?)
        ON CONFLICT DO
            UPDATE SET  example_url=COALESCE(?, example_url), is_spam=?,
                        from_spam_list=?, manually_reviewed=?;",
        )
        .bind(domain.as_str())
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
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
        manually_reviewed: bool,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO urls(url, is_spam, from_spam_list, manually_reviewed)
            VALUES (?, ?, ?, ?)
        ON CONFLICT DO
            UPDATE SET is_spam=?, from_spam_list=?, manually_reviewed=?;",
        )
        .bind(url.as_str())
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
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
    /// Returns true if it was indeed marked as sus.
    pub async fn mark_sus(&self, url: &Url, mut domain: Option<&Domain>) -> Result<bool, Error> {
        // Check current stance in the database.
        if let Some(is_spam_in_db) = self.is_spam(url, domain).await? {
            if is_spam_in_db == IsSpam::Yes {
                // Nothing needs to be done, it's already banned.
                return Ok(false);
            }
        };

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

        Ok(true)
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

    /// Get a URL, and its database table and ID, for review, and its state in the database.
    pub async fn get_url_for_review(&self) -> Result<Option<(Url, &str, i64, IsSpam)>, Error> {
        // Get the mutex. It'll be unlocked at the end of the function
        // automatically due to RAII.
        let _the_mutex = self.review_lock.lock();

        // We heard you like database queries UwU
        let db_result: Option<(Url, IsSpam, i64, bool)> = sqlx::query(
            "SELECT * FROM
                (
                    SELECT url, is_spam, rowid, 1 AS from_urls_table,
                    manually_reviewed, last_sent_to_review
                    FROM urls
                    WHERE from_spam_list=0
                UNION
                    SELECT COALESCE(example_url, domain) AS url, is_spam,
                    rowid, 0 AS from_urls_table,
                    manually_reviewed, last_sent_to_review
                    FROM domains
                    WHERE from_spam_list=0
                )
            ORDER BY manually_reviewed, is_spam DESC, last_sent_to_review LIMIT 1;",
        )
        .map(|row: SqliteRow| {
            (
                parse_url_like_telegram(row.get("url")).expect("Database has invalid URL data!"),
                IsSpam::from(row.get::<u8, _>("is_spam")),
                row.get::<i64, _>("rowid"),
                row.get::<bool, _>("from_urls_table"),
            )
        })
        .fetch_optional(&self.pool)
        .await?;

        let Some((url, is_spam, rowid, from_urls_table)) = db_result else {
            // Well dang.
            return Ok(None);
        };

        // Write the time at which this entry was sent to review...
        {
            let db_query = if from_urls_table {
                "UPDATE urls SET last_sent_to_review=? WHERE rowid=?;"
            } else {
                "UPDATE domains SET last_sent_to_review=? WHERE rowid=?;"
            };

            // Mark this URL or domain in the database as sent to review.
            let time = Utc::now();

            sqlx::query(db_query)
                .bind(time)
                .bind(rowid.to_string())
                .execute(&self.pool)
                .await?;
        }

        let table_name = match from_urls_table {
            false => "domains",
            true => "urls",
        };

        // Pass it on.
        Ok(Some((url, table_name, rowid, is_spam)))
    }

    /// Get a URL from a database table name and rowid.
    pub async fn get_url_from_table_and_rowid(
        &self,
        table: &str,
        rowid: i64,
    ) -> Result<Option<(Url, Option<Domain>)>, Error> {
        match table {
            "domains" => {
                sqlx::query("SELECT domain, example_url FROM domains WHERE rowid=?")
                    .bind(rowid)
                    .map(|row: SqliteRow| {
                        let example_url: Option<&str> = row.get("example_url");
                        let example_url = example_url.map(|x| {
                            parse_url_like_telegram(x).expect("Unparsable example URL in database!")
                        });

                        let domain: &str = row.get("domain");
                        let domain_url = parse_url_like_telegram(domain)
                            .expect("Unparsable domain as URL in database!");
                        let domain =
                            Domain::from_url(&domain_url).expect("Unparsable domain in database!");
                        (example_url.unwrap_or(domain_url), Some(domain))
                    })
                    .fetch_optional(&self.pool)
                    .await
            }
            "urls" => {
                sqlx::query("SELECT url FROM urls WHERE rowid=?")
                    .bind(rowid)
                    .map(|row: SqliteRow| {
                        let url: &str = row.get("url");
                        let url =
                            parse_url_like_telegram(url).expect("Unparsable URL in database!");

                        (url, None)
                    })
                    .fetch_optional(&self.pool)
                    .await
            }
            _ => Ok(None),
        }
    }

    /// Remove a domain from the database, if it exists.
    pub async fn remove_domain(&self, domain: &Domain) -> Result<(), Error> {
        sqlx::query("DELETE FROM domains WHERE domain=?;")
            .bind(domain.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove a URL from the database, if it exists.
    pub async fn remove_url(&self, url: &Url) -> Result<(), Error> {
        sqlx::query("DELETE FROM urls WHERE url=?;")
            .bind(url.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn read_review_response(&self, response: &ReviewResponse) -> Result<(), Error> {
        match response {
            ReviewResponse::Skip => (),
            ReviewResponse::UrlSpam(domain, url) => {
                self.add_url(url, IsSpam::Yes, false, true).await?;
                // Implicitly this means that this URL's domain isn't spam.
                if let Some(domain) = domain {
                    self.remove_domain(domain).await?;
                }
            }
            ReviewResponse::DomainSpam(domain, url) => {
                self.add_domain(domain, Some(url), IsSpam::Yes, false, true)
                    .await?;
                // Implicitly this means that this specific URL is also spam,
                // as part of this domain.
                self.remove_url(url).await?;
            }
            ReviewResponse::NotSpam(domain, url) => {
                // Neither domain nor URL are spam.

                // Only write the response to entries that exist.
                // One of them is bound to exist:
                // the one that the review question was made from lol

                if self.is_url_spam(url).await?.is_some() {
                    self.add_url(url, IsSpam::No, false, true).await?;
                }
                if let Some(domain) = domain {
                    if self.is_domain_spam(domain).await?.is_some() {
                        self.add_domain(domain, Some(url), IsSpam::No, false, true)
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }
}
