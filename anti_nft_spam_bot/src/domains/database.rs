use std::{str::FromStr, sync::atomic::AtomicBool};

pub use sqlx::Error;
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
    Executor, Row, Sqlite,
};
use url::Url;

use super::types::{Domain, IsSpam};

type Pool = sqlx::Pool<Sqlite>;
const DB_PATH: &str = "sqlite:spam_domains.sqlite";
static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

pub struct Database {
    pool: Pool,
}

impl Database {
    pub async fn new() -> Result<Database, Error> {
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
        pool.execute(sqlx::query(
            "
                CREATE TABLE IF NOT EXISTS domains (
                    domain TEXT PRIMARY KEY NOT NULL COLLATE NOCASE,
                    example_url TEXT NULL,
                    is_spam INTEGER NOT NULL,
                    last_sent_to_review TEXT NULL
                ) STRICT;",
        ))
        .await?;

        Ok(Database { pool })
    }

    /// Check if a domain is a spam domain or not, according to the database.
    /// Returns [`None`] if it's not in the database.
    pub async fn is_domain_spam(&self, domain: &Domain) -> Result<Option<IsSpam>, Error> {
        sqlx::query("SELECT is_spam FROM domains WHERE domain=?;")
            .bind(domain.as_str())
            .map(|row: SqliteRow| IsSpam::from(row.get::<u8, _>("is_spam")))
            .fetch_optional(&self.pool)
            .await
    }

    /// Inserts a domain into the database and tag it as spam or not.
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
