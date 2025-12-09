use std::{
    str::FromStr,
    sync::{atomic::AtomicBool, Arc},
};

use chrono::Utc;
use futures_util::{stream::BoxStream, TryStreamExt};
use sqlx::{
    error::ErrorKind,
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
    Executor, Row, Transaction,
};
use teloxide::types::{ChatId, MediaGroupId, MessageId};
use url::Url;

use crate::{misc::parse_url_like_telegram, sanitized_url::SanitizedUrl, types::UrlDesignation};

pub use sqlx::Error;

mod types;
pub use types::*;

type Pool = sqlx::Pool<Sqlite>;
const DB_PATH: &str = "sqlite:anti_nft_spam_bot.sqlite";
static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

/// The database.
pub struct Database {
    /// The connection pool.
    pool: Pool,
}

impl Database {
    /// Create a new database.
    pub fn new() -> impl std::future::Future<Output = Result<Arc<Database>, Error>> + Send {
        Self::new_by_path(DB_PATH, true)
    }

    /// Create a new database for testing.
    #[cfg(test)]
    pub fn new_test() -> impl std::future::Future<Output = Result<Arc<Database>, Error>> + Send {
        Database::new_by_path("sqlite::memory:", false)
    }

    /// Create a new database with specified path. Will check if it's a unique database if `unique`
    /// is set.
    async fn new_by_path(path: &str, unique: bool) -> Result<Arc<Database>, Error> {
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
                    .expect("SQLite connect options should be valid")
                    .pragma("cache_size", "-32768")
                    .foreign_keys(true) // Already  default, but doesn't hurt being explicit.
                    .busy_timeout(std::time::Duration::from_secs(600)),
            )
            .await?;

        // URLS:
        // id (i64, unique primary key)
        // host (text, a host of a `SanitizedUrl`)
        // path (text, a path of a `SanitizedUrl`)
        // query (text, a query of a `SanitizedUrl`, can be empty to mean no query)
        // param_count (i64, amount of params in the URL query (stored in `url_params`); for example,"?a&b=50&c" is 3 params)
        // original_url (text, original URL this sanitized URL is derived from.
        //               NOT GUARANTEED TO BE THE SAME URL!!!)
        // designation (u8, representing a value in the enum `UrlDesignation`)
        // manually_reviewed (0 for no, 1 for yes)
        pool.execute(
            "CREATE TABLE IF NOT EXISTS urls (
                id INTEGER PRIMARY KEY NOT NULL,
                host TEXT NOT NULL,
                path TEXT NOT NULL CHECK (SUBSTR(path, 1, 1)='/'),
                query TEXT NOT NULL,
                param_count INTEGER NOT NULL,
                original_url TEXT NOT NULL,
                designation INTEGER NOT NULL,
                manually_reviewed INTEGER NOT NULL,
                UNIQUE (host, path, query)
            ) STRICT;",
        )
        .await?;

        // URL_PARAMS:
        // url_id (i64, references `id` of table `urls`)
        // param (text, a percent-encoded URL param itself; for example, "v=dQw4w9WgXcQ")
        pool.execute(
            "CREATE TABLE IF NOT EXISTS url_params (
                url_id INTEGER NOT NULL,
                param TEXT NOT NULL,
                UNIQUE (url_id, param),
                FOREIGN KEY (url_id) REFERENCES urls(id)
            ) STRICT;",
        )
        .await?;

        // HIDE_DELETES:
        //      Admins of chats listed here asked to hide
        //      bot's notifications about deleting a message.
        // chat_id (unique primary key, i64)
        pool.execute(sqlx::query(
            "CREATE TABLE IF NOT EXISTS hide_deletes (
                    chat_id INTEGER PRIMARY KEY NOT NULL
            ) STRICT;",
        ))
        .await?;

        // LAST_DELETED_ALBUM_ID:
        // chat_id (unique primary key, i64)
        // media_group_id (text, album ID of the last deleted message)
        pool.execute(sqlx::query(
            "CREATE TABLE IF NOT EXISTS last_deleted_album_id (
                chat_id INTEGER PRIMARY KEY NOT NULL,
                media_group_id TEXT NOT NULL
            ) STRICT;",
        ))
        .await?;

        // review_queue:
        // id (i64, unique primary key)
        // sanitized_url (text, full `SanitizedUrl`)
        // original_url (text, full original URL with no lowercasing)
        // last_sent_to_review (date+time in UTC timezone in ISO 8601 format)
        pool.execute(
            "CREATE TABLE IF NOT EXISTS review_queue (
                id INTEGER PRIMARY KEY NOT NULL,
                sanitized_url TEXT NOT NULL UNIQUE,
                original_url TEXT NOT NULL,
                last_sent_to_review TEXT NULL,
                UNIQUE (sanitized_url)
            ) STRICT;",
        )
        .await?;

        // SUS_LINK_SIGHTINGS:
        //      List of messages where a link on review was sighted.
        //      Used to delete all of them if the link is marked as spam.
        // chat_id (i64)
        // message_id (i32 (because telegram bot api is just like that))
        // sender_name (text)
        // url_id (i64, references rowid of table `review_queue`)
        pool.execute(sqlx::query(
            "CREATE TABLE IF NOT EXISTS sus_link_sightings (
                    chat_id INTEGER NOT NULL,
                    message_id INTEGER NOT NULL,
                    sender_name TEXT NOT NULL,
                    url_id INTEGER NOT NULL,
                    UNIQUE (chat_id, message_id, url_id),
                    FOREIGN KEY (url_id) REFERENCES review_queue(id)
                ) STRICT;",
        ))
        .await?;

        // REVIEW_KEYBOARDS:
        // chat_id (i64)
        // message_id (i32 (because telegram bot api is just like that))
        // url_id (i64, references rowid of table `review_queue`)
        pool.execute(sqlx::query(
            "CREATE TABLE IF NOT EXISTS review_keyboards (
                    chat_id INTEGER NOT NULL,
                    message_id INTEGER NOT NULL,
                    url_id INTEGER NOT NULL,
                    UNIQUE (chat_id, message_id),
                    FOREIGN KEY (url_id) REFERENCES review_queue(id)
                    ) STRICT;",
        ))
        .await?;

        // Two automated indices suggested by sqlite3_expert
        let _ = pool
            .execute(sqlx::query(
                "CREATE INDEX url_params_idx ON url_params(param);",
            ))
            .await;
        let _ = pool
            .execute(sqlx::query(
                "CREATE INDEX urls_host_path_id_idx ON urls(host, path, id DESC);",
            ))
            .await;

        Ok(Arc::new(Database { pool }))
    }

    /// If the input URL has no query, provide empty string for that argument.
    ///
    /// # Panics
    ///
    /// Panics if an invalid [`UrlDesignation`] is found in the database.
    async fn get_url_exact_destructured(
        executor: impl Executor<'_, Database = Sqlite>,
        host: &str,
        path: &str,
        query: &str,
        manual_reviews_only: bool,
    ) -> Result<Option<UrlInfoShort>, Error> {
        assert!(
            path.starts_with('/'),
            "Provided path must correspond to one of a URL"
        );

        let Some(row) = sqlx::query(
            "SELECT id, param_count, designation, manually_reviewed FROM urls
            WHERE host=$1 AND path=$2 AND query=$3 AND ($4 = 0 OR manually_reviewed = $4);",
        )
        .bind(host)
        .bind(path)
        .bind(query)
        .bind(manual_reviews_only)
        .fetch_optional(executor)
        .await?
        else {
            return Ok(None);
        };
        let id: i64 = row.get(0);
        let param_count: i64 = row.get(1);
        let designation = UrlDesignation::try_from(row.get::<u8, _>(2))
            .expect("Invalid URL designation found in database!");
        let manually_reviewed: bool = row.get(3);

        Ok(Some(UrlInfoShort {
            id,
            param_count,
            designation,
            manually_reviewed,
        }))
    }

    /// # Panics
    ///
    /// Panics if an invalid [`UrlDesignation`] is found in the database.
    pub fn get_url_exact<'a>(
        &'a self,
        url: &'a SanitizedUrl,
    ) -> impl std::future::Future<Output = Result<Option<UrlInfoShort>, Error>> + Send + 'a {
        Self::get_url_exact_destructured(
            &self.pool,
            url.host_str(),
            url.path(),
            url.query().unwrap_or(""),
            false,
        )
    }

    /// # Panics
    ///
    /// Panics if an invalid [`UrlDesignation`] is found in the database.
    async fn get_url_inexact_with_query(
        &self,
        url: &SanitizedUrl,
        manual_reviews_only: bool,
    ) -> Result<Option<UrlInfoShort>, Error> {
        // The query is needed a bit later in the code, but it's a good idea to let `.expect` panic
        // here early if needed.
        let query = url
            .query()
            .expect("This function expects URLs with query to be passed");

        // Try to find an exact match first real quick?
        if let Some(exact_match) = self.get_url_exact(url).await? {
            return Ok(Some(exact_match));
        }

        let params = query.split('&');
        let param_count = params.clone().count();

        // The idea behind this query is:
        // 1. Inner join urls with params
        // 2. Filter by host and path.
        // 3. If `exact` function argument is true, filter by exact amount of parameter count too.
        // 4. Filter params to only those that appear in input URL.
        // 5. Group params by URL ID; COUNT(*) is now the amount of matched params per matched URL
        // 6. Filter URLs for which the amount of matching params is not equal to amount of
        //    total params that URL has. This excludes URLs with more params than input.
        // 7. Order filtered URLs by param count; the one matching the most wins.
        //    If there's a conflict... ¯\_ (ツ)_/¯

        let sql_query_str = {
            // SQLx doesn't do array inserts of any kinds yet, so this is the best we can do for
            // now with SQLite, aside from maybe making a temp table and inserting each param into
            // it.

            let sql_query_template = "
            SELECT
                urls.id,
                urls.param_count,
                urls.designation,
                urls.manually_reviewed
            FROM
                url_params,
                urls
            WHERE
                urls.id=url_params.url_id AND
                urls.host=$1 AND
                urls.path=$2 AND
                ($3 == 0 OR urls.manually_reviewed == $3) AND
                param IN (!!!THE_PARAMS!!!)
            GROUP BY urls.id HAVING COUNT(*) == urls.param_count
            ORDER BY urls.param_count DESC
            LIMIT 1;";

            // We want to replace "!!!THE_PARAMS!!!" with something like "$4, $5, $6", with one
            // number for each param.

            let (pre_params, post_params) = sql_query_template
                .split_once("!!!THE_PARAMS!!!")
                .expect("The params must exist in the string");

            let mut sql_query = String::with_capacity(sql_query_template.len());
            sql_query.push_str(pre_params);

            let mut pushed_a_param = false;
            for i in 0..param_count {
                use std::fmt::Write;

                if pushed_a_param {
                    sql_query.push(',');
                }
                write!(sql_query, "${}", i + 4).expect("Writing to a String never fails");
                pushed_a_param = true;
            }

            sql_query.push_str(post_params);

            sql_query
        };

        let mut sql_query = sqlx::query(&sql_query_str)
            .bind(url.as_ref().host_str())
            .bind(url.as_ref().path())
            .bind(manual_reviews_only);

        for param in params {
            sql_query = sql_query.bind(param);
        }

        let Some(row) = sql_query.fetch_optional(&self.pool).await? else {
            return Ok(None);
        };

        let id: i64 = row.get(0);
        let param_count: i64 = row.get(1);
        let designation = UrlDesignation::try_from(row.get::<u8, _>(2))
            .expect("Invalid URL designation found in database!");
        let manually_reviewed: bool = row.get(3);

        Ok(Some(UrlInfoShort {
            id,
            param_count,
            designation,
            manually_reviewed,
        }))
    }

    async fn get_url_inexact_assuming_no_query(
        &self,
        url: &SanitizedUrl,
        manual_reviews_only: bool,
    ) -> Result<Option<UrlInfoShort>, Error> {
        for (host, path) in url.destructure() {
            if let Some(result) =
                Self::get_url_exact_destructured(&self.pool, host, path, "", manual_reviews_only)
                    .await?
            {
                return Ok(Some(result));
            }
        }

        // Nothing matched. Oops.
        Ok(None)
    }

    /// Find and get short info for a URL designations entry matching the given URL, if any.
    pub async fn get_url(
        &self,
        url: &SanitizedUrl,
        manual_reviews_only: bool,
    ) -> Result<Option<UrlInfoShort>, Error> {
        // Try matching on query.
        if url.as_ref().query().is_some() {
            if let Some(result) = self
                .get_url_inexact_with_query(url, manual_reviews_only)
                .await?
            {
                return Ok(Some(result));
            }
        }

        // Nothing found or no query. Either way,
        self.get_url_inexact_assuming_no_query(url, manual_reviews_only)
            .await
    }

    /// Find and get short info from the URL designations table for an entry with this ID, if any.
    ///
    /// # Panics
    ///
    /// Panics if an invalid [`UrlDesignation`] is found in the database.
    #[allow(unused)] // Used in tests, actually.
    pub async fn get_url_by_id_short(&self, id: i64) -> Result<Option<UrlInfoShort>, Error> {
        let Some(row) =
            sqlx::query("SELECT param_count, designation, manually_reviewed FROM urls WHERE id=?;")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(None);
        };
        let param_count: i64 = row.get(0);
        let designation = UrlDesignation::try_from(row.get::<u8, _>(1))
            .expect("Invalid URL designation found in database!");
        let manually_reviewed: bool = row.get(2);

        Ok(Some(UrlInfoShort {
            id,
            param_count,
            designation,
            manually_reviewed,
        }))
    }

    /// Find and get full info from the URL designations table for an entry with this ID, if any.
    ///
    /// # Panics
    ///
    /// Panics if an invalid [`UrlDesignation`], [`Url`], or [`SanitizedUrl`] is found in the database.
    pub async fn get_url_by_id_full(&self, id: i64) -> Result<Option<UrlInfoFull>, Error> {
        let Some(row) = sqlx::query(
            "SELECT
                urls.host,
                urls.path,
                urls.query,
                urls.param_count,
                urls.original_url,
                urls.designation,
                urls.manually_reviewed
            FROM
                urls
            WHERE urls.id=?;",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        // Extract to variables.
        let host: &str = row.get(0);
        let path: &str = row.get(1);
        let query: &str = row.get(2);
        let param_count: i64 = row.get(3);
        let original_url: &str = row.get(4);
        let designation: u8 = row.get(5);
        let manually_reviewed: bool = row.get(6);

        // Now combine to concrete types.
        // Sanitized URL can NOT be derived from original URL.
        // Original URL might be a whole ass
        // different thing the review is made from.
        let sanitized_url = SanitizedUrl::from_str(&format!("https://{host}{path}?{query}"))
            .expect("Invalid sanitized URL found in database!");
        let original_url =
            Url::from_str(original_url).expect("Invalid original URL found in database!");

        //let (sanitized_url, original_url) = SanitizedUrl::from_str_with_original(original_url)
        //    .expect("Invalid URL found in database!");
        let designation = UrlDesignation::try_from(designation)
            .expect("Invalid URL designation found in database!");

        Ok(Some(UrlInfoFull {
            short: UrlInfoShort {
                id,
                param_count,
                designation,
                manually_reviewed,
            },
            sanitized_url,
            original_url,
        }))
    }

    /// Find and get full info for a URL designations entry matching the given URL, if any.
    pub async fn get_url_full(
        &self,
        url: &SanitizedUrl,
        manual_reviews_only: bool,
    ) -> Result<Option<UrlInfoFull>, Error> {
        match self.get_url(url, manual_reviews_only).await? {
            Some(short) => self.get_url_by_id_full(short.id).await,
            None => Ok(None),
        }
    }

    /// Returns ID of the inserted URL.
    async fn insert_url_unchecked(
        transaction: &mut Transaction<'_, Sqlite>,
        sanitized_url: &SanitizedUrl,
        original_url: &Url,
        designation: UrlDesignation,
        manually_reviewed: bool,
    ) -> Result<i64, Error> {
        let params = sanitized_url
            .as_ref()
            .query()
            .into_iter()
            .flat_map(|x| x.split('&'));
        let param_count = params.clone().count();

        let new_id = sqlx::query(
            "INSERT INTO urls (
                host,
                path,
                query,
                param_count,
                original_url,
                designation,
                manually_reviewed)
            VALUES (?, ?, ?, ?, ?, ?, ?)
                ",
        )
        .bind(sanitized_url.host_str())
        .bind(sanitized_url.as_ref().path())
        .bind(sanitized_url.query().unwrap_or(""))
        .bind(param_count.cast_signed() as i64)
        .bind(original_url.as_str())
        .bind(designation as u8)
        .bind(manually_reviewed)
        .execute(&mut **transaction)
        .await?
        .last_insert_rowid();

        if param_count > 0 {
            let mut query = String::from("INSERT INTO url_params(url_id, param) VALUES ");
            for i in 0..param_count {
                use std::fmt::Write;
                write!(query, "({new_id}, ?)").expect("Writing to a String never fails");
                if i != param_count - 1 {
                    query.push(',');
                }
            }
            query.push(';');

            let mut query = sqlx::query(&query);

            for param in params {
                query = query.bind(param);
            }

            query.execute(&mut **transaction).await?;
        }

        Ok(new_id)
    }

    async fn update_url_unchecked(
        transaction: &mut Transaction<'_, Sqlite>,
        id: i64,
        designation: UrlDesignation,
        manually_reviewed: bool,
    ) -> Result<(), Error> {
        sqlx::query("UPDATE urls SET designation=?, manually_reviewed=? WHERE id=?;")
            .bind(designation as u8)
            .bind(manually_reviewed)
            .bind(id)
            .execute(&mut **transaction)
            .await?;

        Ok(())
    }

    /// Insert this into the database, or update an existing entry if one exists.
    ///
    /// You most likely want to call [`crate::actions::insert_or_update_url_with_log`] instead.
    ///
    /// If there is an entry already present in the database, then it's possible no change is
    /// enacted. Specifically, that happens if the new info is not manually reviewed but existing
    /// info is (i.e. an automatic review would be overwriting an old one; in this case, a warning
    /// is emitted), or if both old and new designation and manual review status match.
    pub async fn insert_or_update_url(
        &self,
        sanitized_url: &SanitizedUrl,
        original_url: &Url,
        designation: UrlDesignation,
        manually_reviewed: bool,
    ) -> Result<InsertOrUpdateResult, Error> {
        // Just try inserting as is! If it fails, we'll get a UNIQUE violation error.

        let mut trans = self.pool.begin().await?;

        let insert_result = Self::insert_url_unchecked(
            &mut trans,
            sanitized_url,
            original_url,
            designation,
            manually_reviewed,
        )
        .await;

        match insert_result {
            Ok(new_id) => {
                // nice.
                trans.commit().await?;
                return Ok(InsertOrUpdateResult::Inserted { new_id });
            }
            Err(Error::Database(e)) if e.kind() == ErrorKind::UniqueViolation => {
                // An entry exists. Continue to code below.
            }
            Err(e) => {
                // Some other error. Uh-oh!
                // Dropping trans rolls the transaction back.
                return Err(e);
            }
        }

        // If we're here, that means a unique violation has happened i.e. an entry exists.
        // ...Find it???

        let exact_match = Self::get_url_exact_destructured(
            &mut *trans,
            sanitized_url.host_str(),
            sanitized_url.path(),
            sanitized_url.query().unwrap_or(""),
            false,
        )
        .await?
        .expect("Quantum state URL detected!! What?!?!");

        // Ok yes good. Update it?

        if !manually_reviewed && exact_match.manually_reviewed() {
            // This will overwrite a manually reviewed entry with an automatically determined
            // one. Bad!
            log::warn!(
                "Automatic review tried to overwrite data on manual review for {}",
                sanitized_url.as_str()
            );
            return Ok(InsertOrUpdateResult::NoChange {
                existing_info: exact_match,
            });
        }

        if designation == exact_match.designation() {
            // Both old and new have the same designation. At this point, the only change
            // that could be enacted is changing the "manually reviewed" flag.
            if manually_reviewed == exact_match.manually_reviewed() {
                // ...But even that matches. No change can be enacted.
                return Ok(InsertOrUpdateResult::NoChange {
                    existing_info: exact_match,
                });
            }
        }

        // Above checks passed. This will enact a change.
        Self::update_url_unchecked(&mut trans, exact_match.id, designation, manually_reviewed)
            .await?;

        // Actually apply this lmao
        trans.commit().await?;

        Ok(InsertOrUpdateResult::Updated {
            old_info: exact_match,
        })
    }

    /// If the provided URL is found in the database, removes it and returns [`Some`]`(`[`UrlInfoShort`])`
    /// describing the past entry, otherwise returns [`None`]
    ///
    /// You most likely want to call [`crate::actions::remove_url_with_log`] instead.
    pub async fn remove_url(&self, url: &SanitizedUrl) -> Result<Option<UrlInfoShort>, Error> {
        let mut trans = self.pool.begin().await?;

        let Some(info) = Self::get_url_exact_destructured(
            &mut *trans,
            url.host_str(),
            url.path(),
            url.query().unwrap_or(""),
            false,
        )
        .await?
        else {
            return Ok(None);
        };

        sqlx::query("DELETE FROM url_params WHERE url_id=?")
            .bind(info.id)
            .execute(&mut *trans)
            .await?;

        sqlx::query("DELETE FROM urls WHERE id=?")
            .bind(info.id)
            .execute(&mut *trans)
            .await?;

        trans.commit().await?;

        Ok(Some(info))
    }

    /// Gets whether or not admins of this chat want the bot to not show
    /// notifications about deleting a message.
    ///
    /// True if they don't want them to show, false otherwise.
    pub async fn get_hide_deletes(&self, chat_id: ChatId) -> Result<bool, Error> {
        sqlx::query("SELECT 1 FROM hide_deletes WHERE chat_id=?")
            .bind(chat_id.0)
            .fetch_optional(&self.pool)
            .await
            .map(|x| x.is_some())
    }

    /// Sets whether or not admins of this chat want the bot to not show notifications about
    /// deleting a message. Returns the previous state.
    pub async fn set_hide_deletes(&self, chat_id: ChatId, hide: bool) -> Result<bool, Error> {
        if hide {
            sqlx::query(
                "INSERT INTO hide_deletes (chat_id)
                    VALUES (?)
                    ON CONFLICT DO NOTHING;",
            )
            .bind(chat_id.0)
            .execute(&self.pool)
            .await
            .map(|x| x.rows_affected() == 0) // If true, this means it was set already.
        } else {
            sqlx::query("DELETE FROM hide_deletes WHERE chat_id=?;")
                .bind(chat_id.0)
                .execute(&self.pool)
                .await
                .map(|x| x.rows_affected() > 0) // If true, this means it was set previously.
        }
    }

    /// Inform the database of the ID of the last album which's message was deleted within a chat.
    ///
    /// Used in conjunction with [`Self::get_last_deleted_album_id`].
    pub async fn set_last_deleted_album_id(
        &self,
        chat_id: ChatId,
        album_id: &MediaGroupId,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO last_deleted_album_id (chat_id, media_group_id)
            VALUES ($1, $2)
            ON CONFLICT DO UPDATE SET media_group_id=$2;",
        )
        .bind(chat_id.0)
        .bind(&album_id.0)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get the last deleted album ID in this chat. Used to delete other messages in the same
    /// album.
    ///
    /// Used in conjunction with [`Self::set_last_deleted_album_id`].
    pub async fn get_last_deleted_album_id(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<MediaGroupId>, Error> {
        sqlx::query("SELECT media_group_id FROM last_deleted_album_id WHERE chat_id=? LIMIT 1;")
            .bind(chat_id.0)
            .map(|row: SqliteRow| MediaGroupId(row.get(0)))
            .fetch_optional(&self.pool)
            .await
    }

    /// Send this URL into the review queue.
    pub async fn send_to_review(
        &self,
        sanitized_url: &SanitizedUrl,
        original_url: &Url,
    ) -> Result<SendToReviewResult, Error> {
        // A naive implementation of this method would be:
        //
        // 1. Check if there's URL already in the database due to which this should be rejected.
        // 2. If not, insert into review queue.
        //
        // But if other code inserts a new URL inbetween these two steps, a
        // time-of-check-time-of-use bug might occur. Not a big deal, but worth avoiding.
        //
        // So, fancy-pants plan:
        // 1. Create a transaction.
        // 2. Insert into review queue in the transaction; on conflict, reject.
        //    This write-locks the database, so new URLs can be inserted.
        // 3. Check if there's a conflicting URL in the database right now. If so, rollback and
        //    reject.
        //
        // Shouldn't cause a TOCTOU in this case, I think.

        let mut trans = self.pool.begin().await?;

        let result = sqlx::query(
            "INSERT INTO review_queue
                (sanitized_url, original_url)
            VALUES
                (?, ?)
            ON CONFLICT DO NOTHING;",
        )
        .bind(sanitized_url.as_str())
        .bind(original_url.as_str())
        .execute(&mut *trans)
        .await?;

        if result.rows_affected() == 0 {
            // Dropping a transaction rolls it back.
            return Ok(SendToReviewResult::AlreadyOnReview);
        }

        // Check for conflicting existing entries.
        // Note: the check is done on the main database connection.
        // This is fine with SQLite because the transaction made above is blocking all writes.
        if let Some(existing) = self.get_url_full(sanitized_url, false).await? {
            if existing.designation() == UrlDesignation::Spam {
                // If this marks it as spam, reject.
                return Ok(SendToReviewResult::AlreadyInDatabase(existing));
            }

            if existing.manually_reviewed() && existing.sanitized_url() == sanitized_url {
                // If it's a perfect match and manually reviewed, reject.
                return Ok(SendToReviewResult::AlreadyInDatabase(existing));
            }
        }

        // None! Commit transaction.
        trans.commit().await?;

        Ok(SendToReviewResult::Sent {
            review_entry_id: result.last_insert_rowid(),
        })
    }

    /// Returns database ID of the URL sent on review, and the URL itself.
    ///
    /// # Panics
    ///
    /// Panics if an invalid [`Url`] or [`SanitizedUrl`] is found in the database.
    pub async fn get_url_for_review(&self) -> Result<Option<(i64, SanitizedUrl, Url)>, Error> {
        Ok(sqlx::query(
            "UPDATE review_queue
            SET last_sent_to_review=?
            WHERE id in
                (SELECT id FROM review_queue ORDER BY last_sent_to_review LIMIT 1)
            RETURNING id, sanitized_url, original_url;",
        )
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?
        .map(|row| {
            (
                row.get(0),
                SanitizedUrl::from_str(row.get(1)).expect("Invalid URL found in database!"),
                Url::parse(row.get(2)).expect("Invalid URL found in database!"),
            )
        }))
    }

    /// Inform the database that this URL was sighted in this message in this chat and with this
    /// name of the sender.
    ///
    /// This is done to later get them with [`Self::pop_review_link_sightings`] and remove them
    /// if the review concludes that the link is spam.
    pub async fn link_sighted(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        sender_name: &str,
        link: &SanitizedUrl,
    ) -> Result<(), Error> {
        // Check if this is a sus link; if so, write down this sighting.

        let result = sqlx::query(
            "INSERT INTO sus_link_sightings
                (chat_id, message_id, sender_name, url_id)
            VALUES
                (?, ?, ?, (SELECT id FROM review_queue WHERE sanitized_url=?))
            ON CONFLICT DO NOTHING;",
        )
        .bind(chat_id.0)
        .bind(message_id.0)
        .bind(sender_name)
        .bind(link.as_str())
        .execute(&self.pool)
        .await;

        match result {
            Err(Error::Database(e)) if e.kind() == ErrorKind::NotNullViolation => {
                // This means the SELECT statement has returned nothing/NULL,
                // which means this URL is not on review. That's fine, just ignore.
            }
            x => {
                x?;
            }
        }

        Ok(())
    }

    /// Delete a link from review with this ID.
    ///
    /// If there are existing sightings or keyboards for this URL, an error of
    /// [`ErrorKind::ForeignKeyViolation`] is returned.
    pub async fn delete_from_review(&self, id: i64) -> Result<(), Error> {
        sqlx::query("DELETE FROM review_queue WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Remove and return all sightings of a URL in the review queue at this ID.
    ///
    /// A sighting is a chat ID, a message ID, and name of the sender.
    ///
    /// If no matching URL is found, [`Error::RowNotFound`] is returned.
    pub fn pop_review_link_sightings(
        &self,
        review_entry_id: i64,
    ) -> BoxStream<'_, Result<(ChatId, MessageId, String), Error>> {
        sqlx::query(
            "DELETE FROM sus_link_sightings WHERE url_id=? RETURNING chat_id, message_id, sender_name;",
        )
        .bind(review_entry_id)
        .map(|row: SqliteRow| {
            (
                ChatId(row.get(0)),
                MessageId(row.get(1)),
                row.get::<String, _>(2),
            )
        })
        .fetch(&self.pool)
    }

    /// Find up to one entry still in the review queue for which the best match in the URL
    /// designations table is the an entry with this ID.
    pub async fn find_one_matching_review_queue_entry(
        &self,
        url_entry_to_match_id: i64,
    ) -> Result<Option<i64>, Error> {
        let mut stream = sqlx::query(
            "SELECT
                id,
                sanitized_url
            FROM review_queue;",
        )
        .map(|row: SqliteRow| {
            (
                row.get::<i64, _>(0),
                SanitizedUrl::from_str(row.get(1)).expect("Invalid SanitizedUrl in database!"),
            )
        })
        .fetch(&self.pool);

        while let Some((review_entry_id, sanitized_url)) = stream.try_next().await? {
            let Some(info) = self.get_url(&sanitized_url, false).await? else {
                // No existing entry. Probably still in review.
                continue;
            };

            if info.id() == url_entry_to_match_id {
                // That's it!
                return Ok(Some(review_entry_id));
            }
        }

        // None.
        Ok(None)
    }

    /// Inform the database that a review keyboard was made for a review queue entry at this ID,
    /// and that it's a message at this message ID and chat ID.
    pub async fn review_keyboard_made(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        review_entry_id: i64,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO review_keyboards
            (chat_id, message_id, url_id)
            VALUES ($1, $2, $3)
            ON CONFLICT DO UPDATE SET url_id=$3;",
        )
        .bind(chat_id.0)
        .bind(message_id.0)
        .bind(review_entry_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Inform the database that there is no longer a review keyboard at this message ID and chat
    /// ID.
    pub async fn review_keyboard_removed(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<(), Error> {
        sqlx::query("DELETE FROM review_keyboards WHERE chat_id=? AND message_id=?;")
            .bind(chat_id.0)
            .bind(message_id.0)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Remove review keyboards for this review queue entry and return them one by one.
    pub fn pop_review_keyboards(
        &self,
        review_entry_id: i64,
    ) -> BoxStream<'_, Result<(ChatId, MessageId), Error>> {
        sqlx::query("DELETE FROM review_keyboards WHERE url_id=? RETURNING chat_id, message_id")
            .bind(review_entry_id)
            .map(|row: SqliteRow| (ChatId(row.get(0)), MessageId(row.get(1))))
            .fetch(&self.pool)
    }

    /// Get total count of review queue entries.
    pub fn get_review_count(&self) -> impl std::future::Future<Output = Result<u32, Error>> + '_ {
        sqlx::query("SELECT COUNT(*) FROM review_queue;")
            .map(|row: SqliteRow| row.get(0))
            .fetch_one(&self.pool)
    }

    /// Imports data from the old, pre-rewrite version of this bot.
    ///
    /// For database schema of the old bot, see:
    /// <https://github.com/Architector4/telegram-bot-stuff/blob/6205ce670f6625a0754e04d534a16b137122d3ff/anti_nft_spam_bot/src/database/mod.rs>
    #[allow(unused)]
    pub async fn import_from_old_database(self: &Arc<Self>) -> Result<(), Error> {
        enum IsSpamOld {
            No = 0,
            Yes = 1,
            Maybe = 2,
        }
        impl From<u8> for IsSpamOld {
            fn from(value: u8) -> Self {
                use IsSpamOld::*;
                match value {
                    value if value == No as u8 => No,
                    value if value == Yes as u8 => Yes,
                    value if value == Maybe as u8 => Maybe,
                    _ => panic!("Unknown value: {value}"),
                }
            }
        }

        async fn receiver_task(
            database: Arc<Database>,
            receiver: flume::Receiver<(SanitizedUrl, Url, UrlDesignation)>,
        ) {
            while let Ok((sanitized_url, url, designation)) = receiver.recv() {
                database
                    .insert_or_update_url(&sanitized_url, &url, designation, false)
                    .await
                    .expect("Failed to insert into database!");
            }
        }

        let (sender, receiver) = flume::bounded(64);

        let receiver_task = tokio::spawn(receiver_task(self.clone(), receiver));

        let oldpool = SqlitePoolOptions::new()
            .max_connections(32)
            .connect_with(
                SqliteConnectOptions::from_str("sqlite:spam_domains.sqlite")
                    .expect("SQLite connect options should be valid")
                    .pragma("cache_size", "-32768")
                    .foreign_keys(true) // Already  default, but doesn't hurt being explicit.
                    .busy_timeout(std::time::Duration::from_secs(600)),
            )
            .await?;

        let oldpool_for_hide_deletes = oldpool.clone();
        let database_for_hide_deletes = self.clone();

        let hide_deletes_task = tokio::spawn(async move {
            let mut hide_deletes_stream = sqlx::query("SELECT chatid FROM hide_deletes")
                .map(|row: SqliteRow| ChatId(row.get(0)))
                .fetch(&oldpool_for_hide_deletes);

            while let Some(chatid) = hide_deletes_stream
                .try_next()
                .await
                .expect("Old database died!")
            {
                database_for_hide_deletes
                    .set_hide_deletes(chatid, true)
                    .await
                    .expect("Database died!");
            }
        });

        let old_domains_stream = sqlx::query("SELECT domain, example_url, is_spam FROM domains;")
            .map(|row: SqliteRow| {
                let domain =
                    SanitizedUrl::from_str(row.get(0)).expect("Invalid domain in database!");
                let example_url =
                    parse_url_like_telegram(row.get(1)).expect("Invalid example URL in database!");
                let is_spam = IsSpamOld::from(row.get::<u8, _>(2));

                (domain, example_url, is_spam)
            })
            .fetch(&oldpool);

        let old_urls_stream = sqlx::query("SELECT url, is_spam FROM urls")
            .map(|row: SqliteRow| {
                let (sanitized_url, url) = SanitizedUrl::from_str_with_original(row.get(0))
                    .expect("Invalid example URL in database!");
                let is_spam = IsSpamOld::from(row.get::<u8, _>(1));

                (sanitized_url, url, is_spam)
            })
            .fetch(&oldpool);

        let mut old_urls_chain =
            futures_util::StreamExt::chain(old_domains_stream, old_urls_stream);

        let mut counter = 0usize;

        while let Some((sanitized_url, url, is_spam)) = old_urls_chain.try_next().await? {
            let designation = match is_spam {
                IsSpamOld::Yes => UrlDesignation::Spam,
                IsSpamOld::No => UrlDesignation::NotSpam,
                IsSpamOld::Maybe => continue,
            };

            sender
                .send((sanitized_url, url, designation))
                .expect("Send channel died!");

            counter += 1;

            if counter.is_multiple_of(10000) {
                log::info!("Migrated {counter} URLs from old database...");
            }
        }

        log::info!("Done sending URLs for insertion...");
        drop(sender);

        receiver_task.await.expect("Receiver task failed!");
        log::info!("Waiting for hide deletes sending to be done...");
        hide_deletes_task.await.expect("Hide deletes task failed!");

        log::info!("Old database imported.");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use UrlDesignation::*;

    // Convenience testing methods for Database
    impl Database {
        /// Returns IDs of an inexact and an exact match.
        async fn get_result(&self, url: &str) -> (Option<i64>, Option<i64>) {
            let url: SanitizedUrl = url.parse().unwrap();
            let inexact = self.get_url(&url, false).await.unwrap().map(|x| x.id());
            let exact = self.get_url_exact(&url).await.unwrap().map(|x| x.id());

            (inexact, exact)
        }

        async fn insert(&self, url: &str) -> i64 {
            let (sanitized, original) = SanitizedUrl::from_str_with_original(url).unwrap();
            self.insert_or_update_url(&sanitized, &original, NotSpam, true)
                .await
                .unwrap()
                .id()
        }
    }

    async fn new_db() -> Arc<Database> {
        let db = Database::new_test().await.unwrap();

        // Tests hardcode URL IDs here.
        assert_eq!(1, db.insert("ftp://amogus.com/?b&a&a&c").await);
        assert_eq!(2, db.insert("http://amogus.com/?a&d").await);
        assert_eq!(3, db.insert("http://amogus.com/?e&a&b&c&d&e").await);
        assert_eq!(4, db.insert("ftp://amogus.com/").await);
        assert_eq!(5, db.insert("ftp://amogus.com/testpath/woot/").await);

        db
    }

    #[tokio::test]
    async fn create_db() {
        new_db().await;
    }

    /// Should match exactly to 1
    #[tokio::test]
    async fn match_params_exact() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/?a&c&b").await;
        assert_eq!(inexact, Some(1));
        assert_eq!(exact, Some(1));
    }

    /// Has extraneous params but should match to 2
    #[tokio::test]
    async fn match_params_with_extraneous() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/?a&d&c").await;
        assert_eq!(inexact, Some(2));
        assert_eq!(exact, None);
    }

    /// Has params that make it match both 1 and 2; the match with more params should win
    #[tokio::test]
    async fn match_params_with_multiple_matches() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/?b&a&c&d").await;
        assert_eq!(inexact, Some(1));
        assert_eq!(exact, None);
    }

    /// Has params but does not match anything with them; should match the URL without params.
    #[tokio::test]
    async fn match_params_none() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/?b&a").await;
        assert_eq!(inexact, Some(4));
        assert_eq!(exact, None);
    }

    /// Has no params, but should be exact matches.
    #[tokio::test]
    async fn match_no_params_exact() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/").await;
        assert_eq!(inexact, Some(4));
        assert_eq!(exact, Some(4));
    }

    /// Has no params, but should be exact matches.
    #[tokio::test]
    async fn match_no_params_exact_with_longer_path() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/testpath/woot/").await;
        assert_eq!(inexact, Some(5));
        assert_eq!(exact, Some(5));
    }

    /// Has no exact match, but should eventually descend and match 4.
    #[tokio::test]
    async fn match_no_params_inexact() {
        let db = new_db().await;
        let (inexact, exact) = db.get_result("https://amogus.com/testpath/").await;
        assert_eq!(inexact, Some(4));
        assert_eq!(exact, None);
    }

    /// Has no exact match, but should eventually descend and match 4.
    #[tokio::test]
    async fn match_no_params_inexact_longer() {
        let db = new_db().await;
        let (inexact, exact) = db
            .get_result("https://amogus.com/testpath/woot/aawagggga")
            .await;
        assert_eq!(inexact, Some(5));
        assert_eq!(exact, None);
    }

    /// Has no exact match, but should eventually descend and match 4.
    #[tokio::test]
    async fn match_no_params_inexact_with_input_params() {
        let db = new_db().await;
        let (inexact, exact) = db
            .get_result("https://amogus.com/aawagga/amogus/?woot=3")
            .await;
        assert_eq!(inexact, Some(4));
        assert_eq!(exact, None);
    }

    #[tokio::test]
    async fn get_url_short_with_params() {
        let db = new_db().await;
        let UrlInfoShort {
            id,
            param_count,
            designation,
            manually_reviewed,
        } = db.get_url_by_id_short(3).await.unwrap().unwrap();
        assert_eq!(id, 3);
        assert_eq!(param_count, 5);
        assert_eq!(designation, NotSpam);
        assert!(manually_reviewed);
    }

    #[tokio::test]
    async fn get_url_short_with_path() {
        let db = new_db().await;
        let UrlInfoShort {
            id,
            param_count,
            designation,
            manually_reviewed,
        } = db.get_url_by_id_short(5).await.unwrap().unwrap();
        assert_eq!(id, 5);
        assert_eq!(param_count, 0);
        assert_eq!(designation, NotSpam);
        assert!(manually_reviewed);
    }

    #[tokio::test]
    async fn update_result() {
        let db = new_db().await;
        let (sanitized_url, original_url) =
            SanitizedUrl::from_str_with_original("example.com").unwrap();
        let result = db
            .insert_or_update_url(
                &sanitized_url,
                &original_url,
                UrlDesignation::NotSpam,
                false,
            )
            .await
            .unwrap();
        assert!(matches!(
            result,
            InsertOrUpdateResult::Inserted { new_id: _ }
        ));
        let result = db
            .insert_or_update_url(&sanitized_url, &original_url, UrlDesignation::Spam, true)
            .await
            .unwrap();
        assert!(matches!(
            result,
            InsertOrUpdateResult::Updated {
                old_info: UrlInfoShort {
                    id: _,
                    param_count: _,
                    designation: UrlDesignation::NotSpam,
                    manually_reviewed: false
                }
            }
        ));
        let result = db
            .insert_or_update_url(&sanitized_url, &original_url, UrlDesignation::Spam, true)
            .await
            .unwrap();
        assert!(matches!(
            result,
            InsertOrUpdateResult::NoChange {
                existing_info: UrlInfoShort {
                    id: _,
                    param_count: _,
                    designation: UrlDesignation::Spam,
                    manually_reviewed: true
                }
            }
        ));
    }

    #[tokio::test]
    async fn get_url_full_with_params() {
        let db = new_db().await;
        let UrlInfoFull {
            short:
                UrlInfoShort {
                    id,
                    param_count,
                    designation,
                    manually_reviewed,
                },
            sanitized_url,
            original_url,
        } = db.get_url_by_id_full(3).await.unwrap().unwrap();

        assert_eq!(id, 3);
        assert_eq!(param_count, 5);
        assert_eq!(designation, NotSpam);
        assert!(manually_reviewed);
        assert_eq!(sanitized_url.as_str(), "https://amogus.com/?a&b&c&d&e");
        assert_eq!(original_url.as_str(), "http://amogus.com/?e&a&b&c&d&e");
    }

    #[tokio::test]
    async fn get_url_full_with_path() {
        let db = new_db().await;
        let UrlInfoFull {
            short:
                UrlInfoShort {
                    id,
                    param_count,
                    designation,
                    manually_reviewed,
                },
            sanitized_url,
            original_url,
        } = db.get_url_by_id_full(5).await.unwrap().unwrap();

        assert_eq!(id, 5);
        assert_eq!(param_count, 0);
        assert_eq!(designation, NotSpam);
        assert!(manually_reviewed);
        assert_eq!(sanitized_url.as_str(), "https://amogus.com/testpath/woot");
        assert_eq!(original_url.as_str(), "ftp://amogus.com/testpath/woot/");
    }

    #[tokio::test]
    async fn review_push_results() {
        let db = new_db().await;

        let urls = SanitizedUrl::from_str_with_original("amogus.com").unwrap();
        let result = db.send_to_review(&urls.0, &urls.1).await.unwrap();
        assert!(matches!(result, SendToReviewResult::AlreadyInDatabase(_)));

        let urls = SanitizedUrl::from_str_with_original("somenewurl.com").unwrap();

        let result = db.send_to_review(&urls.0, &urls.1).await.unwrap();
        assert!(matches!(
            result,
            SendToReviewResult::Sent { review_entry_id: 1 }
        ));

        let result = db.send_to_review(&urls.0, &urls.1).await.unwrap();
        assert!(matches!(result, SendToReviewResult::AlreadyOnReview));
    }

    #[tokio::test]
    async fn review_push_get() {
        let db = new_db().await;

        let urls = SanitizedUrl::from_str_with_original("somenewurl.com").unwrap();
        let send_result = db.send_to_review(&urls.0, &urls.1).await.unwrap();
        assert!(matches!(
            send_result,
            SendToReviewResult::Sent { review_entry_id: 1 }
        ));

        let get_result = db.get_url_for_review().await.unwrap().map(|x| (x.1, x.2));
        assert_eq!(get_result, Some(urls.clone()));

        let get_result = db.get_url_for_review().await.unwrap().map(|x| (x.1, x.2));
        assert_eq!(get_result, Some(urls));
    }

    /// When requesting a URL for review, it should return the oldest sent, so all URLs are cycled
    /// through.
    #[tokio::test]
    async fn review_push_get_multiple() {
        let db = new_db().await;

        let urls_a = SanitizedUrl::from_str_with_original("a.com").unwrap();
        let send_result = db.send_to_review(&urls_a.0, &urls_a.1).await.unwrap();
        assert!(matches!(
            send_result,
            SendToReviewResult::Sent { review_entry_id: 1 }
        ));

        let urls_b = SanitizedUrl::from_str_with_original("b.com").unwrap();
        let send_result = db.send_to_review(&urls_b.0, &urls_b.1).await.unwrap();
        assert!(matches!(
            send_result,
            SendToReviewResult::Sent { review_entry_id: 2 }
        ));

        let get_a = db.get_url_for_review().await.unwrap().map(|x| (x.1, x.2));
        assert_eq!(get_a, Some(urls_a.clone()));

        let get_b = db.get_url_for_review().await.unwrap().map(|x| (x.1, x.2));
        assert_eq!(get_b, Some(urls_b.clone()));

        let get_a = db.get_url_for_review().await.unwrap().map(|x| (x.1, x.2));
        assert_eq!(get_a, Some(urls_a.clone()));

        let get_b = db.get_url_for_review().await.unwrap().map(|x| (x.1, x.2));
        assert_eq!(get_b, Some(urls_b.clone()));
    }

    /// Basically test that it doesn't crash lol
    #[tokio::test]
    async fn sus_link_sighting() {
        let db = new_db().await;
        let urls = SanitizedUrl::from_str_with_original("example.com").unwrap();

        // This URL is not in the database. Should do nothing.
        db.link_sighted(ChatId(0), MessageId(0), "hi", &urls.0)
            .await
            .unwrap();

        let send_result = db.send_to_review(&urls.0, &urls.1).await.unwrap();
        assert!(matches!(
            send_result,
            SendToReviewResult::Sent { review_entry_id: 1 }
        ));

        // This URL is now in the database. Should do something now.
        db.link_sighted(ChatId(0), MessageId(0), "hi", &urls.0)
            .await
            .unwrap();
    }

    // Simple boolean logic, but yeag.
    #[tokio::test]
    async fn hide_deletes() {
        let db = new_db().await;

        let chat_id = ChatId(1312);

        // false by default
        assert!(!db.get_hide_deletes(chat_id).await.unwrap());
        assert!(!db.get_hide_deletes(chat_id).await.unwrap());
        // setting to false does nothing
        assert!(!db.set_hide_deletes(chat_id, false).await.unwrap());
        assert!(!db.set_hide_deletes(chat_id, false).await.unwrap());
        // setting to true, well, sets to true
        assert!(!db.set_hide_deletes(chat_id, true).await.unwrap());
        assert!(db.set_hide_deletes(chat_id, true).await.unwrap());
        assert!(db.get_hide_deletes(chat_id).await.unwrap());
        assert!(db.get_hide_deletes(chat_id).await.unwrap());
        // setting to false sets to false lol
        assert!(db.set_hide_deletes(chat_id, false).await.unwrap());
        assert!(!db.set_hide_deletes(chat_id, false).await.unwrap());
    }

    #[tokio::test]
    async fn last_deleted_album_id() {
        let db = new_db().await;
        let chat_id = ChatId(1312);

        let album_id = MediaGroupId(String::from("amogus"));

        assert_eq!(db.get_last_deleted_album_id(chat_id).await.unwrap(), None);

        db.set_last_deleted_album_id(chat_id, &album_id)
            .await
            .unwrap();

        assert_eq!(
            db.get_last_deleted_album_id(chat_id).await.unwrap(),
            Some(album_id)
        );
    }
}
