mod list_watcher;

use std::{
    collections::HashSet,
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
use teloxide::{types::ChatId, Bot};
use tokio::sync::{watch, Mutex, Notify};
use url::Url;

use crate::{
    parse_url_like_telegram,
    spam_checker::SPAM_CHECKER_VERSION,
    types::{MarkSusResult, ReviewResponse},
};

use super::types::{Domain, IsSpam};

type Pool = sqlx::Pool<Sqlite>;
const DB_PATH: &str = "sqlite:spam_domains.sqlite";
static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

pub struct Database {
    pool: Pool,
    drop_watch: (watch::Sender<()>, watch::Receiver<()>),
    // Mutexes are bad. However, this will only be used for reviews,
    // which can only be done by a few people in the control chat.
    review_lock: Mutex<()>,
    /// A list of domains that are currently being visited by other tasks.
    /// For the reason the Mutex is used, see code of [`Self::domain_visit_debounce`]
    // I'd make this a std::sync::Mutex but Rust incorrectly assumes it lives after
    // the drop and doesn't let it compile lol
    domains_currently_being_visited: Mutex<HashSet<Domain>>,
    /// A [`Notify`] used to wake up tasks waiting on other tasks to visit some domain.
    domains_visit_notify: Notify,
}

impl Database {
    pub fn new(bot: Bot) -> impl std::future::Future<Output = Result<Arc<Database>, Error>> + Send {
        Self::new_by_path(bot, DB_PATH, true)
    }

    /// Create a new database with specified path. Will check if it's a unique database if `unique`
    /// is set. If `bot` is provided, it will also ingest the `spam_website_list.txt` file and
    /// watch it for changes.
    async fn new_by_path(
        bot: impl Into<Option<Bot>>,
        path: &str,
        unique: bool,
    ) -> Result<Arc<Database>, Error> {
        if unique {
            assert!(
                !WAS_CONSTRUCTED.swap(true, std::sync::atomic::Ordering::SeqCst),
                "Second database was constructed. This is not allowed."
            );
        }

        if !Sqlite::database_exists(path).await.unwrap_or(false) {
            Sqlite::create_database(path).await?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(32)
            .connect_with(
                SqliteConnectOptions::from_str(path)
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
        // spam_checker_version (version of this program this was determined at)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS domains (
                    domain TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    example_url TEXT NULL,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL,
                    manually_reviewed INTEGER NOT NULL DEFAULT 0,
                    from_spam_list INTEGER NOT NULL DEFAULT 0,
                    spam_checker_version INTEGER NOT NULL DEFAULT 0
                ) STRICT;",
        ))
        .await?;

        // URLS:
        // url (unique primary key, string)
        // is_spam (0 for no, 1 for yes, 2 for unknown and needs review)
        // last_sent_to_review (date+time in UTC timezone in ISO 8601 format)
        // manually_reviewed (0 for no, 1 for yes)
        // from_spam_list (0 for no, 1 for yes)
        // spam_checker_version (version of this program this was determined at)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS urls (
                    url TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL,
                    manually_reviewed INTEGER NOT NULL DEFAULT 0,
                    from_spam_list INTEGER NOT NULL DEFAULT 0,
                    spam_checker_version INTEGER NOT NULL DEFAULT 0
                ) STRICT;",
        ))
        .await?;

        // HIDE_DELETES:
        //      An admin of chats listed here asked to hide
        //      bot's notifications about deleting a message.
        // chatid (unique primary key, i64)
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS hide_deletes (
                    chatid INTEGER PRIMARY KEY NOT NULL
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
        let _ = sqlx::query(
            "ALTER TABLE domains
        ADD COLUMN spam_checker_version INTEGER NOT NULL DEFAULT 0;",
        )
        .execute(&pool)
        .await;
        let _ = sqlx::query(
            "ALTER TABLE urls
        ADD COLUMN spam_checker_version INTEGER NOT NULL DEFAULT 0;",
        )
        .execute(&pool)
        .await;

        let db_arc = Arc::new(Database {
            pool,
            review_lock: Mutex::new(()),
            drop_watch: watch::channel(()),
            domains_currently_being_visited: Mutex::new(HashSet::with_capacity(4)),
            domains_visit_notify: Notify::new(),
        });

        if let Some(bot) = bot.into() {
            // Spawn the watcher.
            tokio::spawn(list_watcher::watch_list(bot, db_arc.clone()));
        }

        Ok(db_arc)
    }

    /// Check if a domain is a spam domain or not, according to the database.
    /// Returns [`None`] if it's not in the database.
    ///
    /// Note that [`Self::is_url_spam`] should take priority over this,
    /// unless its return result is [`IsSpam::Maybe`].
    ///
    /// "No" and "Maybe" results that were automatically determined by
    /// an old spam checker are ignored unless `return_old_checker_results` is set to true.
    pub async fn is_domain_spam(
        &self,
        domain: &Domain,
        return_old_checker_results: bool,
    ) -> Result<Option<IsSpam>, Error> {
        // The "NOT" condition is to exclude results that says anything other than `IsSpam::Yes`
        // and are automatically determined by an older spam check version.
        // We DON'T want to delete those, because they should still be useful for review.
        sqlx::query(
            "SELECT is_spam FROM domains
            WHERE domain=? AND
                NOT (
                    is_spam!=1 AND
                    from_spam_list=0 AND
                    spam_checker_version<?
                    );",
        )
        .bind(domain.as_str())
        .bind(if return_old_checker_results {
            0
        } else {
            SPAM_CHECKER_VERSION
        })
        .map(|row: SqliteRow| IsSpam::from(row.get::<u8, _>("is_spam")))
        .fetch_optional(&self.pool)
        .await
    }

    /// Check if a URL is a spam URL or not, according to the database.
    /// Returns [`None`] if it's not in the database.
    ///
    /// Note that this should take priority over [`Self::is_domain_spam`],
    /// unless this function's return result is [`IsSpam::Maybe`].
    ///
    /// "No" and "Maybe" results that were automatically determined by
    /// an old spam checker are ignored unless `return_old_checker_results` is set to true.
    pub async fn is_url_spam(
        &self,
        url: &Url,
        return_old_checker_results: bool,
    ) -> Result<Option<IsSpam>, Error> {
        // The "NOT" condition is to exclude results that says anything other than `IsSpam::Yes`
        // and are automatically determined by an older spam check version.
        // We DON'T want to delete those, because they should still be useful for review.
        sqlx::query(
            "SELECT is_spam FROM urls
            WHERE url=? AND
                NOT (
                    is_spam!=1 AND
                    from_spam_list=0 AND
                    spam_checker_version<?
                    );",
        )
        .bind(url.as_str())
        .bind(if return_old_checker_results {
            0
        } else {
            SPAM_CHECKER_VERSION
        })
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
    ///
    /// "No" and "Maybe" results that were automatically determined by
    /// an old spam checker are ignored unless `return_old_checker_results` is set to true.
    pub async fn is_spam(
        &self,
        url: &Url,
        domain: impl Into<Option<&Domain>>,
        return_old_checker_results: bool,
    ) -> Result<Option<IsSpam>, Error> {
        let mut domain = domain.into();
        // Look for URL match...
        let url_result = self.is_url_spam(url, return_old_checker_results).await?;

        if let Some(IsSpam::Yes) = url_result {
            return Ok(url_result);
        }

        // If no provided domain, try to get one from the URL.
        // Otherwise, use provided domain, to not do an extraneous allocation.
        let domain_inner;
        if domain.is_none() {
            domain_inner = Domain::from_url(url);
            domain = domain_inner.as_ref();
        }

        // Look for domain match...
        let domain_result = if let Some(domain) = domain {
            self.is_domain_spam(domain, return_old_checker_results)
                .await?
        } else {
            None
        };

        // Pick the most condemning one.
        let result = IsSpam::pick_most_condemning(url_result, domain_result);
        Ok(result)
    }

    /// Inserts a domain into the database and tags it as spam or not.
    /// Overwrites the domain if it already exists.
    pub async fn add_domain(
        &self,
        domain: &Domain,
        example_url: impl Into<Option<&Url>>,
        is_spam: IsSpam,
        from_spam_list: bool,
        manually_reviewed: bool,
    ) -> Result<(), Error> {
        let example_url = example_url.into();
        sqlx::query(
            "INSERT INTO domains(
                domain,
                example_url,
                is_spam,
                from_spam_list,
                manually_reviewed,
                spam_checker_version)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT DO UPDATE SET
                example_url=COALESCE(?, example_url),
                is_spam=?,
                from_spam_list=?,
                manually_reviewed=?,
                spam_checker_version=?;",
        )
        .bind(domain.as_str())
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
        .bind(SPAM_CHECKER_VERSION)
        // On conflict...
        .bind(example_url.map(Url::as_str))
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
        .bind(SPAM_CHECKER_VERSION)
        .execute(&self.pool)
        .await?;

        // If we know for a fact that this URL and its domain is
        // spam, we don't need an entry in the `urls` table for it.
        if let Some(url) = example_url {
            sqlx::query("DELETE FROM urls WHERE url=? AND is_spam=?;")
                .bind(url.as_str())
                .bind::<u8>(is_spam.into())
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Mark a domain as maybe spam, if it's not already marked as spam
    /// and wasn't manually reviewed. Returns true if anything is actually done.
    ///
    /// Note that this adds a URL entry if one doesn't exist,
    /// even if there's a meaningful domain entry.
    async fn mark_domain_sus(
        &self,
        domain: &Domain,
        example_url: Option<&Url>,
    ) -> Result<bool, Error> {
        let result = sqlx::query(
            "
            INSERT INTO domains(
                domain,
                example_url,
                is_spam,
                spam_checker_version
            ) VALUES (?, ?, 2, ?)
            ON CONFLICT DO
            UPDATE SET
                example_url=COALESCE(?, example_url),
                is_spam=2,
                spam_checker_version=?
            WHERE is_spam=0 AND manually_reviewed=0;",
        )
        .bind(domain.as_str())
        .bind(example_url.map(Url::as_str))
        .bind(SPAM_CHECKER_VERSION)
        .bind(example_url.map(Url::as_str))
        .bind(SPAM_CHECKER_VERSION)
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0;
        Ok(result)
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
            "INSERT INTO urls(
                url,
                is_spam,
                from_spam_list,
                manually_reviewed,
                spam_checker_version)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT DO UPDATE SET
                is_spam=?,
                from_spam_list=?,
                manually_reviewed=?,
                spam_checker_version=?;",
        )
        .bind(url.as_str())
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
        .bind(SPAM_CHECKER_VERSION)
        // On conflict...
        .bind::<u8>(is_spam.into())
        .bind(from_spam_list)
        .bind(manually_reviewed)
        .bind(SPAM_CHECKER_VERSION)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a URL as maybe spam, if it's not already marked as spam
    /// and wasn't manually reviewed. Returns true if anything is actually done.
    ///
    /// Note that this adds a URL entry if one doesn't exist,
    /// even if there's a meaningful domain entry.
    async fn mark_url_sus(&self, url: &Url) -> Result<bool, Error> {
        let result = sqlx::query(
            "
            INSERT INTO urls(
                    url,
                    is_spam,
                    spam_checker_version
            ) VALUES (?, 2, ?)
            ON CONFLICT DO
                UPDATE SET is_spam=2, spam_checker_version=?
                WHERE is_spam=0 AND manually_reviewed=0;",
        )
        .bind(url.as_str())
        .bind(SPAM_CHECKER_VERSION)
        .bind(SPAM_CHECKER_VERSION)
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0;
        Ok(result)
    }

    /// Convenience function to mark both a URL and its domain as maybe spam.
    pub async fn mark_sus(
        &self,
        url: &Url,
        mut domain: Option<&Domain>,
    ) -> Result<MarkSusResult, Error> {
        // We only want to deal with entries in the database that exist.

        // Check the URL one.
        if let Some(is_spam_url) = self.is_url_spam(url, false).await? {
            let result = match is_spam_url {
                IsSpam::Yes => MarkSusResult::AlreadyMarkedSpam,
                IsSpam::Maybe => MarkSusResult::AlreadyMarkedSus,
                IsSpam::No => {
                    let mark_result = self.mark_url_sus(url).await?;
                    if mark_result {
                        MarkSusResult::Marked
                    } else {
                        MarkSusResult::ManuallyReviewedNotSpam
                    }
                }
            };

            return Ok(result);
        }

        // If no provided domain, try to get one from the URL.
        // Otherwise, use provided domain, to not do an extraneous allocation.
        let domain_inner;
        if domain.is_none() {
            domain_inner = Domain::from_url(url);
            domain = domain_inner.as_ref();
        }
        if let Some(domain) = domain {
            // Check the domain one.
            if let Some(is_spam_domain) = self.is_domain_spam(domain, false).await? {
                let result = match is_spam_domain {
                    IsSpam::Yes => MarkSusResult::AlreadyMarkedSpam,
                    IsSpam::Maybe => MarkSusResult::AlreadyMarkedSus,
                    IsSpam::No => {
                        let mark_result = self.mark_domain_sus(domain, Some(url)).await?;
                        if mark_result {
                            MarkSusResult::Marked
                        } else {
                            MarkSusResult::ManuallyReviewedNotSpam
                        }
                    }
                };

                return Ok(result);
            }
        }

        // It is in neither URL nor Domain tables.
        // Add it in as a URL entry.
        self.mark_url_sus(url).await?;
        Ok(MarkSusResult::Marked)
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

    /// Count and return the amount of links left to review.
    pub async fn get_review_count(&self) -> Result<u32, Error> {
        sqlx::query(
            "SELECT SUM(A) FROM
                (
                    SELECT COUNT(*) AS A FROM urls WHERE is_spam=2
                UNION ALL
                    SELECT COUNT(*) AS A FROM domains WHERE is_spam=2
                );",
        )
        .map(|x: SqliteRow| x.get(0))
        .fetch_one(&self.pool)
        .await
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

                // Write the result about the URL unconditionally.
                self.add_url(url, IsSpam::No, false, true).await?;

                if let Some(domain) = domain {
                    // But only write about the domain if it's already in the database lol
                    if self.is_domain_spam(domain, true).await?.is_some() {
                        self.add_domain(domain, Some(url), IsSpam::No, false, true)
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Gets whether or not admins of this chat want the bot to not show
    /// notifications about deleting a message.
    pub async fn get_hide_deletes(&self, chatid: ChatId) -> Result<bool, Error> {
        sqlx::query("SELECT 1 FROM hide_deletes WHERE chatid=?")
            .bind(chatid.0)
            .fetch_optional(&self.pool)
            .await
            .map(|x| x.is_some())
    }

    /// Sets whether or not admins of this chat want the bot to not show
    /// notifications about deleting a message. Returns the previous state.
    pub async fn set_hide_deletes(&self, chatid: ChatId, hide: bool) -> Result<bool, Error> {
        let old_state = self.get_hide_deletes(chatid).await?;

        if old_state == hide {
            // It's already set to that. Do nothing, return true.
            return Ok(hide);
        }

        // Aw, we actually have to do things now :(

        if hide {
            sqlx::query(
                "INSERT INTO hide_deletes (chatid)
                    VALUES (?)
                    ON CONFLICT DO NOTHING;",
            )
            .bind(chatid.0)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("DELETE FROM hide_deletes WHERE chatid=?;")
                .bind(chatid.0)
                .execute(&self.pool)
                .await?;
        }

        Ok(old_state)
    }
}

pub struct DomainVisitDebounceGuard {
    database: Arc<Database>,
    domain: Domain,
}

impl Drop for DomainVisitDebounceGuard {
    fn drop(&mut self) {
        let tokio_handle = tokio::runtime::Handle::current();
        let database = self.database.clone();
        let mut domain = Domain::new_invalid_unchecked();

        std::mem::swap(&mut self.domain, &mut domain);

        tokio_handle.spawn(async move {
            database
                .domains_currently_being_visited
                .lock()
                .await
                .remove(&domain);
            database.domains_visit_notify.notify_waiters();
        });
    }
}

impl Database {
    /// Returns [`DomainVisitDebounceGuard`] if this domain isn't being visited,
    /// or, if it is, blocks until that is done and then returns [`None`].
    pub async fn domain_visit_debounce(
        self: &Arc<Database>,
        domain: Domain,
    ) -> Option<DomainVisitDebounceGuard> {
        // This is set to true if this domain was spotted to be in the process of being visited.
        let mut was_visited = false;

        // Check if it's already being visited on loop until it's no longer being visited.
        loop {
            // Code below looks a bit weird, but it's to avoid a race condition.
            // Consider the following scenario:
            // 1. Task A is currently visiting a domain.
            // 2. Task B runs this function to check it.
            // 3. Task B finds the domain in the hash set in the check below.
            // 4. Task A finishes visiting the domain and removes it from the
            // hash set.
            // 5. Task A then punts the Notify.
            // 6. Task B, awaits the Notify *after* that.
            // 7. Task B locks up until something else happens to punt the Notify.
            //
            // For this reason, task B has to ensure that task A can't punt the Notify
            // after task B checked the hash set but before it awaits the Notify.
            //
            // This is accomplished with the visited_lock: the hash set is locked
            // for checking, then Notify is awaited on, and only then it's unlocked.

            let mut visited_lock = self.domains_currently_being_visited.lock().await;

            let contains = visited_lock.contains(&domain);
            if contains {
                // It's being visited. Wait on notify and check again.
                was_visited = true;
                // Notified immediately starts listening as it is created.
                let notify_waiter = self.domains_visit_notify.notified();
                drop(visited_lock);
                notify_waiter.await;
            } else {
                // It is not or no longer being visited.
                // Add it to the list and return the guard.

                if was_visited {
                    break None;
                }

                visited_lock.insert(domain.clone());
                drop(visited_lock);

                break Some(DomainVisitDebounceGuard {
                    database: self.clone(),
                    domain,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Ret = Result<(), Error>;

    pub fn new_temp() -> impl std::future::Future<Output = Result<Arc<Database>, Error>> + Send {
        Database::new_by_path(None, "sqlite::memory:", false)
    }

    #[tokio::test]
    async fn create_db() -> Ret {
        new_temp().await?;
        Ok(())
    }

    #[tokio::test]
    async fn is_url_spam() -> Ret {
        let db = new_temp().await?;
        let spam: Url = parse_url_like_telegram("amogus.com/badspam").unwrap();

        assert_eq!(db.is_url_spam(&spam, false).await?, None);
        assert_eq!(db.is_spam(&spam, None, false).await?, None);

        db.add_url(&spam, IsSpam::Yes, false, false).await?;
        assert_eq!(db.is_url_spam(&spam, false).await?, Some(IsSpam::Yes));

        let other: Url = parse_url_like_telegram("amogus.com/otherurl").unwrap();
        assert_eq!(db.is_url_spam(&other, false).await?, None);

        let spamdomain: Domain = Domain::from_url(&spam).unwrap();
        // This checks if the domain specifically is a spam, so it will return None.
        assert_eq!(db.is_domain_spam(&spamdomain, false).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn is_domain_spam() -> Ret {
        let db = new_temp().await?;
        let spamurl: Url = parse_url_like_telegram("amogus.com/badspam").unwrap();
        let spamdomain: Domain = Domain::from_url(&spamurl).unwrap();

        assert_eq!(db.is_url_spam(&spamurl, false).await?, None);
        assert_eq!(db.is_domain_spam(&spamdomain, false).await?, None);
        assert_eq!(db.is_spam(&spamurl, None, false).await?, None);
        assert_eq!(db.is_spam(&spamurl, Some(&spamdomain), false).await?, None);

        db.add_domain(&spamdomain, Some(&spamurl), IsSpam::Yes, false, false)
            .await?;
        // This checks if the URL specifically is a spam, so it will return None.
        assert_eq!(db.is_url_spam(&spamurl, false).await?, None);
        assert_eq!(
            db.is_domain_spam(&spamdomain, false).await?,
            Some(IsSpam::Yes)
        );
        assert_eq!(db.is_spam(&spamurl, None, false).await?, Some(IsSpam::Yes));
        assert_eq!(
            db.is_spam(&spamurl, Some(&spamdomain), false).await?,
            Some(IsSpam::Yes)
        );

        let other: Url = parse_url_like_telegram("amogus.com/otherurl").unwrap();
        let otherdomain: Domain = Domain::from_url(&other).unwrap();
        assert_eq!(spamdomain, otherdomain);
        // This checks if the URL specifically is a spam, so it will return None.
        assert_eq!(db.is_url_spam(&other, false).await?, None);
        assert_eq!(db.is_spam(&other, None, false).await?, Some(IsSpam::Yes));
        assert_eq!(
            db.is_spam(&other, Some(&otherdomain), false).await?,
            Some(IsSpam::Yes)
        );
        Ok(())
    }

    #[tokio::test]
    async fn mark_sus_workflow() -> Ret {
        let db = new_temp().await?;
        let link = parse_url_like_telegram("example.com/notspam").unwrap();
        let domain = Domain::from_url(&link).unwrap();

        // Let's say the link is posted in a chat.
        // First, check `crate::spam_checker::check` is run, which defers to the db.
        assert_eq!(db.is_spam(&link, None, false).await?, None);

        // Then, the checker determines it as not spam and adds it to
        // the database.
        db.add_domain(&domain, &link, IsSpam::No, false, false)
            .await
            .expect("Database died!");

        // Then, someone marks it as sus.
        assert_eq!(db.mark_sus(&link, None).await?, MarkSusResult::Marked);

        // Check if this is what it is in the database.
        assert_eq!(db.is_spam(&link, None, false).await?, Some(IsSpam::Maybe));

        // Someone gets it in review...
        let (review_url, review_table, review_id, db_state) =
            db.get_url_for_review().await?.unwrap();
        assert_eq!(review_url, link);
        assert_eq!(db_state, IsSpam::Maybe);

        // They mark it as not spam...
        let from_db = db
            .get_url_from_table_and_rowid(review_table, review_id)
            .await?
            .unwrap();
        assert_eq!(from_db.0, link);
        let response = ReviewResponse::NotSpam(from_db.1, from_db.0);
        db.read_review_response(&response).await?;

        // Someone later posts the link again.
        // Check if this is what it is in the database.
        assert_eq!(db.is_spam(&link, None, false).await?, Some(IsSpam::No));

        // Someone marks it as sus again...
        assert_eq!(
            db.mark_sus(&link, None).await?,
            MarkSusResult::ManuallyReviewedNotSpam
        );
        //db.mark_url_sus(&link).await?;

        //// It should still be not spam.
        //assert_eq!(db.is_spam(&link, None, false).await?, Some(IsSpam::No));

        Ok(())
    }

    #[tokio::test]
    async fn adding_links_actually_adds() -> Ret {
        let url = parse_url_like_telegram("example.com/notspam").unwrap();
        let domain = Domain::from_url(&url).unwrap();

        for spam_status in [IsSpam::No, IsSpam::Maybe, IsSpam::Yes] {
            let db = new_temp().await?;
            db.add_domain(&domain, &url, spam_status, false, false)
                .await?;
            assert_eq!(db.is_spam(&url, &domain, true).await?, Some(spam_status));
            let db = new_temp().await?;
            db.add_url(&url, spam_status, false, false).await?;
            assert_eq!(db.is_spam(&url, &domain, true).await?, Some(spam_status));
        }

        Ok(())
    }

    #[tokio::test]
    async fn review_response_conflicts_with_db() -> Ret {
        // Check all possible cases of `ReviewResponse` in
        // relation to the database's state.
        let url = parse_url_like_telegram("example.com/notspam").unwrap();
        let domain = Domain::from_url(&url).unwrap();

        // Test cases.
        let skip = ReviewResponse::Skip;
        let notspam = ReviewResponse::NotSpam(Some(domain.clone()), url.clone());
        let urlspam = ReviewResponse::UrlSpam(Some(domain.clone()), url.clone());
        let domainspam = ReviewResponse::DomainSpam(domain.clone(), url.clone());

        // Neither URL nor domain is in the database.
        let db = new_temp().await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(notspam.conflicts_with_db(&db).await?);
        assert!(urlspam.conflicts_with_db(&db).await?);
        assert!(domainspam.conflicts_with_db(&db).await?);

        //

        // The URL is marked as not spam.
        let db = new_temp().await?;
        db.add_url(&url, IsSpam::No, false, false).await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(!notspam.conflicts_with_db(&db).await?);
        assert!(urlspam.conflicts_with_db(&db).await?);
        assert!(domainspam.conflicts_with_db(&db).await?);

        // The URL is marked as maybe spam.
        let db = new_temp().await?;
        db.add_url(&url, IsSpam::Maybe, false, false).await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(notspam.conflicts_with_db(&db).await?);
        assert!(urlspam.conflicts_with_db(&db).await?);
        assert!(domainspam.conflicts_with_db(&db).await?);

        // The URL is marked as yes spam.
        let db = new_temp().await?;
        db.add_url(&url, IsSpam::Yes, false, false).await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(notspam.conflicts_with_db(&db).await?);
        assert!(!urlspam.conflicts_with_db(&db).await?);
        assert!(domainspam.conflicts_with_db(&db).await?);

        //

        // The domain is marked as not spam.
        let db = new_temp().await?;
        db.add_domain(&domain, &url, IsSpam::No, false, false)
            .await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(!notspam.conflicts_with_db(&db).await?);
        assert!(urlspam.conflicts_with_db(&db).await?);
        assert!(domainspam.conflicts_with_db(&db).await?);

        // The domain is marked as maybe spam.
        let db = new_temp().await?;
        db.add_domain(&domain, &url, IsSpam::Maybe, false, false)
            .await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(notspam.conflicts_with_db(&db).await?);
        assert!(urlspam.conflicts_with_db(&db).await?);
        assert!(domainspam.conflicts_with_db(&db).await?);

        // The domain is marked as yes spam.
        let db = new_temp().await?;
        db.add_domain(&domain, &url, IsSpam::Yes, false, false)
            .await?;
        assert!(!skip.conflicts_with_db(&db).await?);
        assert!(notspam.conflicts_with_db(&db).await?);
        assert!(urlspam.conflicts_with_db(&db).await?);
        assert!(!domainspam.conflicts_with_db(&db).await?);

        Ok(())
    }

    #[tokio::test]
    async fn marking_telegram_as_spam_by_accident() -> Ret {
        // Scenario:
        // 1. Someone puts a normal non-spam telegram link in review.
        // 2. Admin marks it as not spam.
        // 3. Someone puts a spam telegram link in review.
        // 4. Admin accidentally responds that telegram's entire domain is spam.
        //
        // The make_a_db() function below creates a database in this state.

        let spam: Url = parse_url_like_telegram("t.me/badspam").unwrap();
        let normal: Url = parse_url_like_telegram("t.me/channels").unwrap();
        let tg = parse_url_like_telegram("t.me").unwrap();
        let tgdomain = Domain::from_url(&tg).unwrap();

        /// Make a database with initial state.
        async fn make_a_db() -> Result<Arc<Database>, Error> {
            let db = new_temp().await?;
            let spam: Url = parse_url_like_telegram("t.me/badspam").unwrap();
            let normal: Url = parse_url_like_telegram("t.me/channels").unwrap();
            let tg = parse_url_like_telegram("t.me").unwrap();
            let tgdomain = Domain::from_url(&tg).unwrap();

            // Let's say someone marks Telegram itself as spam on accident.

            // Someone gets the normal link in review...
            assert_eq!(db.mark_sus(&normal, None).await?, MarkSusResult::Marked);

            // Someone gets it in review...
            let (_, review_table, review_id, _) = db.get_url_for_review().await?.unwrap();

            // They mark it as not spam...
            let from_db = db
                .get_url_from_table_and_rowid(review_table, review_id)
                .await?
                .unwrap();
            assert_eq!(from_db.0, normal);
            let response = ReviewResponse::NotSpam(from_db.1, from_db.0);
            db.read_review_response(&response).await?;

            // Someone gets the spam link in review...
            assert_eq!(db.mark_sus(&spam, None).await?, MarkSusResult::Marked);

            // Someone gets it in review...
            let (_, review_table, review_id, _) = db.get_url_for_review().await?.unwrap();

            // They mark the DOMAIN as spam on accident...
            let from_db = db
                .get_url_from_table_and_rowid(review_table, review_id)
                .await?
                .unwrap();
            assert_eq!(from_db.0, spam);
            let response = ReviewResponse::DomainSpam(tgdomain.clone(), from_db.0);
            db.read_review_response(&response).await?;

            // Oh no. The normal link is spam too.
            assert_eq!(
                db.is_spam(&normal, None, false).await.unwrap(),
                Some(IsSpam::Yes)
            );

            // How will our heroes get out of this one? Find out on next episode of...
            Ok(db)
        }

        // Scenario continuation:
        // 5. Admin tries to fix this by marking "t.me" as not spam.
        let db = make_a_db().await?;
        let response = ReviewResponse::NotSpam(Some(tgdomain.clone()), tg);
        db.read_review_response(&response).await?;

        // Normal link shouldn't be spam now.
        assert_eq!(
            db.is_spam(&normal, None, false).await.unwrap(),
            Some(IsSpam::No)
        );
        // In this case the spam link isn't either though.
        assert_eq!(
            db.is_spam(&spam, None, false).await.unwrap(),
            Some(IsSpam::No)
        );

        // Scenario continuation:
        // 5. Admin tries to fix this by marking the spam link as URL spam.
        let db = make_a_db().await?;
        let response = ReviewResponse::UrlSpam(Some(tgdomain), spam.clone());
        db.read_review_response(&response).await?;

        // Normal link shouldn't be spam now.
        assert_eq!(
            db.is_spam(&normal, None, false).await.unwrap(),
            Some(IsSpam::No)
        );
        // In this case the spam link should still be considered spam.
        assert_eq!(
            db.is_spam(&spam, None, false).await.unwrap(),
            Some(IsSpam::Yes)
        );

        Ok(())
    }
}
