use std::{
    sync::{atomic::AtomicBool, Arc, Weak},
    time::Duration,
};

pub mod database;
use arch_bot_commons::useful_methods::BotArchSendMsg;
use database::Database;
use html_escape::encode_text;
use teloxide::{
    payloads::EditMessageTextSetters,
    requests::Requester,
    types::{Message, UserId},
    ApiError, Bot, RequestError,
};
use tokio::{sync::Notify, time::sleep};
use tokio_stream::StreamExt;

use super::Task;
use crate::OWNER_ID;

pub struct Taskman {
    pub db: Arc<Database>,
    bot: Bot,
    // Arc is so that taskman can be dropped independently of notify
    notify: Arc<Notify>,
}

static WAS_CONSTRUCTED: AtomicBool = AtomicBool::new(false);

impl Drop for Taskman {
    fn drop(&mut self) {
        self.notify.notify_waiters();
    }
}

impl Taskman {
    pub async fn new(db: Arc<Database>, bot: Bot) -> Arc<Self> {
        assert!(
            !WAS_CONSTRUCTED.swap(true, std::sync::atomic::Ordering::SeqCst),
            "Second taskman was constructed. This is not allowed."
        );

        #[allow(clippy::let_and_return)]
        let taskman = Arc::new(Self {
            db,
            bot,
            notify: Arc::new(Notify::new()),
        });

        tokio::task::spawn(queue_counter_spinjob(Arc::downgrade(&taskman)));
        tokio::task::spawn(task_completion_spinjob(Arc::downgrade(&taskman), false));
        tokio::task::spawn(task_completion_spinjob(Arc::downgrade(&taskman), true));

        taskman
    }

    pub async fn add_task(
        &self,
        user: Option<UserId>,
        task: Task,
        request_message: &Message,
        queue_response_message: &Message,
    ) -> Result<u32, database::Error> {
        let response = self
            .db
            .add_task(user, task, request_message, queue_response_message)
            .await;

        self.notify.notify_waiters();

        response
    }

    //pub async fn edit_task(
    //    &self,
    //    task: Task,
    //    request_message: &Message,
    //    queue_response_message: &Message,
    //) -> Result<u64, database::Error> {
    //    self.notify.notify_waiters();
    //    self.db
    //        .edit_task(task, request_message, queue_response_message)
    //        .await
    //}
}

pub async fn task_completion_spinjob(taskman: Weak<Taskman>, premium: bool) {
    loop {
        let Some(taskman) = taskman.upgrade() else {
            return;
        };

        // Just in case, *before* we fetch the tasks.
        let notify = taskman.notify.clone();
        let notified = notify.notified();

        let Some(task_data) = taskman.db.grab_task(premium).await.expect("Database died!") else {
            // No tasks. Vacuum the database?
            taskman.db.idle_cleanup().await;
            drop(taskman);
            notified.await;
            continue;
        };

        // Inform the user that we're doing the task.
        let response = task_data.task.produce_queue_message(0);
        let _ = taskman
            .bot
            .edit_message_text(
                task_data.queue_message_chat_id,
                task_data.queue_message_id,
                response,
            )
            .parse_mode(teloxide::types::ParseMode::Html)
            .await;

        // Try up to 3 times
        let mut result = task_data.task.complete_task(&taskman.bot, &task_data).await;
        if result.is_err() {
            result = task_data.task.complete_task(&taskman.bot, &task_data).await;
        }
        if result.is_err() {
            result = task_data.task.complete_task(&taskman.bot, &task_data).await;
        }

        let _ = taskman
            .bot
            .delete_message(task_data.queue_message_chat_id, task_data.queue_message_id)
            .await;

        if let Err(e) = result {
            let mut request_deleted: bool = false;
            if let RequestError::Api(ApiError::Unknown(s)) = &e {
                request_deleted = s.contains("message to reply not found");
            };

            if !request_deleted {
                log::error!(
                    "ERROR when processing task: {:#?}\nTask data: {:#?}",
                    e,
                    task_data
                );
                let _ = taskman
                    .bot
                    .archsendmsg(
                        task_data.message.chat.id,
                        concat!(
                            "An error has occurred while processing this task. ",
                            "The bot's owner will be notified to fix this."
                        ),
                        task_data.message.id,
                    )
                    .await;
                sleep(Duration::from_secs(1)).await;
                if let Err(e) = taskman
                    .bot
                    .archsendmsg(
                        OWNER_ID,
                        encode_text(&format!("ERROR: {:#?}\n\nTask data: {:#?}", e, task_data))
                            .as_ref(),
                        None,
                    )
                    .await
                {
                    log::error!("ERROR when sending the info above to the owner:\n{:#?}", e);
                }
            }
        };

        // Task done. Delete the "edit message", just in case.

        if let Some(new_task_data) = taskman
            .db
            .get_task_by_id(task_data.taskid)
            .await
            .expect("Database died!")
        {
            if let (Some(edit_chat_id), Some(edit_id)) = (
                new_task_data.edit_response_chat_id,
                new_task_data.edit_response_message_id,
            ) {
                let _ = taskman.bot.delete_message(edit_chat_id, edit_id).await;
            }
        }

        taskman
            .db
            .delete_task(task_data.taskid)
            .await
            .expect("Database died!");

        sleep(Duration::from_secs(3)).await;
    }
}

pub async fn queue_counter_spinjob(taskman: Weak<Taskman>) {
    loop {
        sleep(Duration::from_secs(1)).await;
        let Some(taskman) = taskman.upgrade() else {
            return;
        };

        let mut queue = taskman.db.queue_iterator();

        // Just in case, *before* we fetch the tasks.
        let notify = taskman.notify.clone();
        let notified = notify.notified();

        let mut got_a_single_task = false;

        while let Some(taskid) = queue.try_next().await.expect("Database died!") {
            got_a_single_task = true;
            let Some(taskdata) = taskman
                .db
                .get_task_by_id(taskid)
                .await
                .expect("Database died!")
            else {
                continue;
            };

            let Some(queue_size) = taskman
                .db
                .get_queue_size_for_task(taskid)
                .await
                .expect("Database died!")
            else {
                continue;
            };

            let response = taskdata.task.produce_queue_message(queue_size);
            //let now = sqlx::types::chrono::Utc::now().to_string();

            if taskman
                .bot
                .edit_message_text(
                    taskdata.queue_message_chat_id,
                    taskdata.queue_message_id,
                    response,
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await
                .is_err()
            {
                continue;
            }
        }

        if !got_a_single_task {
            // No tasks. Vacuum the database?
            taskman.db.idle_cleanup().await;
            drop(queue);
            drop(taskman);
            notified.await;
        }
        //sleep(Duration::from_secs(5)).await;
    }
}
