use std::{pin::Pin, str::FromStr, sync::atomic::AtomicBool};

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
pub use sqlx::Error;
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Executor, Row, Sqlite,
};
use teloxide::types::{ChatId, Message, MessageId, UserId};
use tokio_stream::Stream;

use crate::{tasks::Task, OWNER_ID};

type Pool = sqlx::Pool<Sqlite>;
const DB_PATH: &str = "sqlite:teco_tools.sqlite";
static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)] // Intentionally allow unused fields here.
#[derive(Debug, Clone)]
pub struct TaskDatabaseInfo {
    pub taskid: i64,
    pub userid: UserId,
    pub task: Task,
    pub message: Message,
    pub queue_message_chat_id: ChatId,
    pub queue_message_id: MessageId,
    pub edit_response_chat_id: Option<ChatId>,
    pub edit_response_message_id: Option<MessageId>,
    pub in_progress: bool,
    pub premium: bool,
    pub delay_processing_until: Option<DateTime<Utc>>,
}

impl TaskDatabaseInfo {
    fn from_sqlite_row(row: SqliteRow) -> TaskDatabaseInfo {
        TaskDatabaseInfo {
            taskid: row.get(0),
            userid: UserId(row.get::<i64, _>(1) as u64),
            task: serde_json::from_str(row.get(2)).unwrap(),
            message: serde_json::from_str(row.get(3)).unwrap(),
            queue_message_chat_id: ChatId(row.get(4)),
            queue_message_id: MessageId(row.get(5)),
            edit_response_chat_id: row.get::<Option<i64>, _>(6).map(ChatId),
            edit_response_message_id: row.get::<Option<i32>, _>(7).map(MessageId),
            in_progress: row.get(8),
            premium: row.get(9),
            delay_processing_until: row.get(10),
        }
    }
}

pub struct Database {
    pool: Pool,
}

impl Database {
    pub async fn new() -> Result<Self, Error> {
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

        // TASKS:
        // taskid (key, i64),
        // userid (i64 because sqlite doesn't support u64; may be NULL),
        // task (task object serialized in RON),
        // message (message that requested the task, serialized in RON;
        //          will also contain all the file hashes and stuff as well as
        //          the replied-to message),
        // request_message_chat_id (i64),
        // request_message_id (i32 (because telegram bot api is just like that)),
        // queue_message_chat_id (i64),
        // queue_message_id (i32 (because telegram bot api is just like that)),
        // edit_response_chat_id (i64, may be NULL),
        // edit_response_message_id (i32 (because telegram bot api is just like that), may be NULL),
        // in_progress (0 for no, 1 for yes),
        // premium (0 for no, 1 for yes),
        // delay_processing_until (date+time in UTC in RFC3339 format),
        // clobbers (u32 bitmask, see Task::clobbers method for details)
        pool.execute(sqlx::query(
            "CREATE TABLE IF NOT EXISTS tasks (
                taskid INTEGER PRIMARY KEY NOT NULL,
                userid INTEGER NULL,
                task TEXT NOT NULL,
                message TEXT NOT NULL,
                request_message_chat_id INTEGER NOT NULL,
                request_message_id INTEGER NOT NULL,
                queue_message_chat_id INTEGER NOT NULL,
                queue_message_id INTEGER NOT NULL,
                edit_response_chat_id INTEGER NULL,
                edit_response_message_id INTEGER NULL,
                in_progress INTEGER NOT NULL,
                premium INTEGER NOT NULL,
                delay_processing_until TEXT NULL,
                clobbers INTEGER NOT NULL DEFAULT 0
            ) STRICT;",
        ))
        .await?;

        // PREMIUM_USERS:
        // userid (key, u64)
        pool.execute(sqlx::query(
            "CREATE TABLE IF NOT EXISTS premium_users (
                userid INTEGER PRIMARY KEY NOT NULL
            ) STRICT;",
        ))
        .await?;

        let _ = sqlx::query("CREATE INDEX tasks_userid ON tasks(userid);")
            .execute(&pool)
            .await;
        let _ = sqlx::query("CREATE INDEX tasks_premium ON tasks(premium);")
            .execute(&pool)
            .await;
        let _ = sqlx::query("CREATE INDEX tasks_in_progress ON tasks(in_progress);")
            .execute(&pool)
            .await;
        let _ = sqlx::query(concat!(
            "CREATE INDEX tasks_request_message ON ",
            "tasks(request_message_chat_id, request_message_id);"
        ))
        .execute(&pool)
        .await;

        // Transparent database migration lololol
        // Will fail harmlessly if the column already exists.
        let _ = pool
            .execute(sqlx::query(
                "ALTER TABLE tasks
                ADD COLUMN delay_processing_until TEXT NULL;",
            ))
            .await;
        let _ = pool
            .execute(sqlx::query(
                "ALTER TABLE tasks
                ADD COLUMN clobbers INTEGER NOT NULL DEFAULT 0;",
            ))
            .await;

        // We're just starting, so nothing could be in progress.
        pool.execute(sqlx::query("UPDATE tasks SET in_progress=0;"))
            .await?;

        let woot = Database { pool };

        woot.idle_cleanup().await;

        Ok(woot)
    }

    #[allow(clippy::cast_possible_wrap)]
    pub async fn is_user_premium(&self, id: UserId) -> Result<bool, Error> {
        if id == OWNER_ID {
            return Ok(true);
        }
        sqlx::query("SELECT 1 FROM premium_users WHERE userid=?;")
            .bind(id.0 as i64)
            .fetch_optional(&self.pool)
            .await
            .map(|x| x.is_some())
    }

    /// Returns the new task's position in queue.
    pub(super) async fn add_task(
        &self,
        user: Option<UserId>,
        task: Task,
        request_message: &Message,
        queue_message: &Message,
        delay_processing_until: Option<DateTime<Utc>>,
    ) -> Result<u32, Error> {
        let task_ser = serde_json::to_string(&task).unwrap();

        let request_message_ser = serde_json::to_string(&request_message).unwrap();
        let request_message_chat_id = request_message.chat.id.0;
        let request_message_id = request_message.id.0;
        let queue_message_chat_id = queue_message.chat.id.0;
        let queue_message_id = queue_message.id.0;

        let premium = if let Some(user) = &request_message.from {
            self.is_user_premium(user.id).await?
        } else {
            false
        };

        let clobbers = task.clobbers();

        let queue_size = self.get_queue_size_raw(premium).await?;

        sqlx::query(
            "INSERT INTO tasks (
                userid,
                task,
                message,
                request_message_chat_id,
                request_message_id,
                queue_message_chat_id,
                queue_message_id,
                in_progress,
                premium,
                delay_processing_until,
                clobbers
            ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?);",
        )
        .bind(user.map(|x| x.0 as i64))
        .bind(task_ser)
        .bind(request_message_ser)
        .bind(request_message_chat_id)
        .bind(request_message_id)
        .bind(queue_message_chat_id)
        .bind(queue_message_id)
        .bind(premium)
        .bind(delay_processing_until)
        .bind(clobbers)
        .execute(&self.pool)
        .await?;

        Ok(queue_size + 1)
    }

    pub async fn idle_cleanup(&self) {
        let _ = sqlx::query("VACUUM;").execute(&self.pool).await;
        let _ = sqlx::query("ANALYZE;").execute(&self.pool).await;
    }

    async fn get_queue_size_raw(&self, premium: bool) -> Result<u32, Error> {
        let count: u32 = sqlx::query("SELECT COUNT(*) FROM tasks WHERE premium=?;")
            .bind(premium)
            .fetch_one(&self.pool)
            .await?
            .get(0);

        Ok(count)
    }

    pub async fn get_queue_size(&self, premium: bool) -> Result<u32, Error> {
        let mut count = self.get_queue_size_raw(premium).await?;
        if premium {
            let non_premium_count = self.get_queue_size_raw(false).await?;
            count = count.min(non_premium_count);
            if non_premium_count > 0 {
                // At least one non-premium task, which may be the one currently running.
                // In this case, presume queue size is at least 1.
                count = count.max(1);
            }
        }

        Ok(count)
    }

    pub async fn get_queue_size_for_task(&self, taskid: i64) -> Result<Option<u32>, Error> {
        sqlx::query(
            "SELECT COUNT(*) FROM tasks
            WHERE
                    tasks.taskid < ?
                AND
                    tasks.premium=(SELECT premium FROM tasks WHERE taskid=?);",
        )
        .bind(taskid)
        .bind(taskid)
        .map(|row: SqliteRow| row.get(0))
        .fetch_optional(&self.pool)
        .await
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn queue_iterator<'a>(
        &'a self,
    ) -> Pin<Box<dyn Stream<Item = Result<i64, Error>> + Send + 'a>> {
        let stream = sqlx::query("SELECT taskid FROM tasks;")
            .map(|row: SqliteRow| row.get(0))
            .fetch(&self.pool);

        stream
    }

    pub async fn get_task_by_id(&self, taskid: i64) -> Result<Option<TaskDatabaseInfo>, Error> {
        sqlx::query(
            "SELECT
                taskid,
                userid,
                task,
                message,
                queue_message_chat_id,
                queue_message_id,
                edit_response_chat_id,
                edit_response_message_id,
                in_progress,
                premium,
                delay_processing_until
            FROM tasks WHERE taskid=?",
        )
        .bind(taskid)
        .map(TaskDatabaseInfo::from_sqlite_row)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn get_task_by_request_message(
        &self,
        request_message: &Message,
    ) -> Result<Option<TaskDatabaseInfo>, Error> {
        let request_message_chat_id = request_message.chat.id.0;
        let request_message_id = request_message.id.0;
        sqlx::query(
            "SELECT
                taskid,
                userid,
                task,
                message,
                queue_message_chat_id,
                queue_message_id,
                edit_response_chat_id,
                edit_response_message_id,
                in_progress,
                premium,
                delay_processing_until
            FROM tasks WHERE request_message_chat_id=? AND request_message_id=?",
        )
        .bind(request_message_chat_id)
        .bind(request_message_id)
        .map(TaskDatabaseInfo::from_sqlite_row)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn task_edit_response(
        &self,
        taskid: i64,
        edit_response: &Message,
    ) -> Result<(), Error> {
        sqlx::query(
            "UPDATE tasks SET
                edit_response_chat_id=?,
                edit_response_message_id=?
            WHERE taskid=?",
        )
        .bind(edit_response.chat.id.0)
        .bind(edit_response.id.0)
        .bind(taskid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn edit_task(
        &self,
        taskid: i64,
        task: &Task,
        request_message: &Message,
    ) -> Result<(), Error> {
        let task_ser = serde_json::to_string(&task).unwrap();
        let request_message_ser = serde_json::to_string(request_message).unwrap();

        sqlx::query(
            "UPDATE tasks SET
                task=?,
                message=?,
                edit_response_chat_id=NULL,
                edit_response_message_id=NULL
            WHERE taskid=?",
        )
        .bind(task_ser)
        .bind(request_message_ser)
        //.bind(edit_response_chat_id.map(|x| x.0))
        //.bind(edit_response_message_id.map(|x| x.0))
        .bind(taskid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete task, due to either completion or cancellation.
    pub async fn delete_task(&self, taskid: i64) -> Result<(), Error> {
        sqlx::query("DELETE FROM tasks WHERE taskid=?;")
            .bind(taskid)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn grab_task(&self, premium: bool) -> Result<Option<TaskDatabaseInfo>, Error> {
        let now = Utc::now();
        // Select a task. Find a fitting one to complete,
        // then set it as in-progress and return its ID.
        let Some(taskid): Option<i64> = sqlx::query(
            "UPDATE tasks SET in_progress = 1
            FROM (
                SELECT
                    min(taskid) AS minid
                FROM
                    tasks
                WHERE
                    in_progress=0 AND
                    premium=? AND
                    COALESCE(delay_processing_until <= ?, 1) AND
                    ( -- Check if this won't clobber with any running task
                        clobbers=0 OR
                        NOT EXISTS
                        (
                            SELECT 1 FROM tasks t2
                                WHERE t2.in_progress=1 AND t2.clobbers & clobbers != 0
                        )
                    )
            ) AS fitting_task
            WHERE
                taskid=fitting_task.minid
            RETURNING taskid",
        )
        .bind(premium)
        .bind(now)
        .map(|row: SqliteRow| row.get(0))
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        self.get_task_by_id(taskid).await
    }

    pub async fn user_has_too_much_tasks(&self, user: Option<UserId>) -> Result<bool, Error> {
        let parallelisms = std::thread::available_parallelism()
            .map(|x| x.get())
            .unwrap_or_default()
            .max(3);

        sqlx::query("SELECT 1 FROM tasks WHERE userid=? GROUP BY userid HAVING COUNT(*) >= ?")
            .bind(user.map(|x| x.0 as i64))
            .bind(parallelisms as i64)
            .fetch_optional(&self.pool)
            .await
            .map(|x| x.is_some())
    }

    pub async fn set_premium(&self, user: UserId, premium: bool) -> Result<(), Error> {
        if premium {
            sqlx::query("INSERT INTO premium_users(userid) VALUES (?) ON CONFLICT DO NOTHING;")
                .bind(user.0 as i64)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query("DELETE FROM premium_users WHERE userid=?;")
                .bind(user.0 as i64)
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    /// Returns how long is left until at least one delayed task's delay expires.
    ///
    /// Returns `None` if there are no delayed tasks,
    /// or [`Duration::ZERO`] if any delayed task's delay has already expired.
    pub async fn time_until_earliest_delayed_task(
        &self,
        premium: bool,
    ) -> Result<Option<std::time::Duration>, Error> {
        let Some(Some(earliest_delayed_task_time)) =
            sqlx::query("SELECT MIN(delay_processing_until) FROM tasks WHERE premium=?;")
                .bind(premium)
                .map(|row: SqliteRow| row.get::<Option<DateTime<Utc>>, _>(0))
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(None);
        };

        let now = Utc::now();

        let time_until = earliest_delayed_task_time - now;

        Ok(Some(
            time_until.to_std().unwrap_or(std::time::Duration::ZERO),
        ))
    }
}
