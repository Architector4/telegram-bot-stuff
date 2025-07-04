use std::{
    sync::{atomic::AtomicBool, Arc, Weak},
    time::Duration,
};

pub mod database;
use arch_bot_commons::{teloxide_retry, useful_methods::BotArchSendMsg};
use chrono::{DateTime, Utc};
use database::Database;
use html_escape::encode_text;
use teloxide::{
    payloads::EditMessageTextSetters,
    requests::Requester,
    sugar::request::RequestLinkPreviewExt,
    types::{Message, UserId},
    ApiError, Bot, RequestError,
};
use tokio::{
    sync::{watch, Notify},
    time::{sleep, timeout},
};
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

        // Closure that spawns a task completion spinjob
        let spawntask = |premium| {
            tokio::task::spawn(task_completion_spinjob(Arc::downgrade(&taskman), premium))
        };

        let parallelisms = std::thread::available_parallelism()
            .map(|x| x.get())
            .unwrap_or(2);

        // Spawn two tasks per each pair of parallelisms, at least once
        for _ in 0..parallelisms.max(2) / 2 {
            spawntask(false);
            spawntask(true);
        }
        if parallelisms % 2 > 0 {
            // If we have an odd amount of parallelisms, spawn an extra task for that one
            spawntask(true);
        }

        taskman
    }

    /// Returns the new task's position in queue, and if it's delayed.
    pub async fn add_task(
        &self,
        user: Option<UserId>,
        task: Task,
        request_message: &Message,
        queue_response_message: &Message,
        delay_processing_until: Option<DateTime<Utc>>,
    ) -> Result<u32, database::Error> {
        let response = self
            .db
            .add_task(
                user,
                task,
                request_message,
                queue_response_message,
                delay_processing_until,
            )
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

        let mut task_data = taskman.db.grab_task(premium).await.expect("Database died!");
        if task_data.is_none() {
            // No task. Try to grab a task from the other queue then.
            task_data = taskman
                .db
                .grab_task(!premium)
                .await
                .expect("Database died!");
        }

        let Some(task_data) = task_data else {
            // No tasks to immediately grab. Check if there's any delayed tasks to wait for,
            // then wait for a notify or such a task.
            let sleep_for = taskman
                .db
                .time_until_earliest_delayed_task(premium)
                .await
                .expect("Database died!");
            drop(taskman);

            if let Some(sleep_for) = sleep_for {
                let _ = timeout(sleep_for, notified).await;
            } else {
                notified.await;
            }
            continue;
        };

        // Inform the user that we're doing the task.
        macro_rules! produce_queue_message {
            ($task: expr, $taskman:expr, $progress: expr) => {
                // Inform the user that we're doing the task.
                let response = $task.produce_queue_message(Some(0), $progress);
                let _ = $taskman
                    .bot
                    .edit_message_text(
                        task_data.queue_message_chat_id,
                        task_data.queue_message_id,
                        response,
                    )
                    .disable_link_preview(true)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await;
            };
        }
        produce_queue_message!(task_data.task, taskman, None);

        let (sender, mut receiver) = watch::channel(String::new());

        let status_updater = {
            let taskman = taskman.clone();
            let task = task_data.task.clone();
            tokio::spawn(async move {
                let mut last_received = String::new();
                receiver.borrow_and_update();
                loop {
                    receiver.borrow_and_update().clone_into(&mut last_received);
                    if !last_received.is_empty() {
                        produce_queue_message!(task, taskman, Some(&last_received));
                        sleep(Duration::from_secs(2)).await;
                    }
                    if receiver.changed().await.is_err() {
                        break;
                    }
                }
            })
        };

        let result = teloxide_retry!(
            task_data
                .task
                .complete_task(sender.clone(), &taskman.bot, &task_data)
                .await
        );

        drop(sender);
        let _ = status_updater.await;

        let _ = taskman
            .bot
            .delete_message(task_data.queue_message_chat_id, task_data.queue_message_id)
            .await;

        if let Err(e) = result {
            let mut request_deleted: bool = false;
            if let RequestError::Api(ApiError::Unknown(s)) = &e {
                // to telegram: ?????????????????????????????
                request_deleted = s.contains("Bad Request: message to reply not found")
                    || s.contains("Bad Request: message to be replied not found");
            };

            if !request_deleted {
                log::error!("ERROR when processing task: {e:#?}\nTask data: {task_data:#?}");
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
                sleep(Duration::from_secs(2)).await;
                if let Err(e) = taskman
                    .bot
                    .archsendmsg(
                        OWNER_ID,
                        encode_text(&format!("ERROR: {e:#?}\n\nTask data: {task_data:#?}"))
                            .as_ref(),
                        None,
                    )
                    .await
                {
                    log::error!("ERROR when sending the info above to the owner:\n{e:#?}");
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

        // If it clobbers anything, poke notify for other workers to retry picking tasks.
        if task_data.task.clobbers() != 0 {
            taskman.notify.notify_waiters();
        }

        sleep(Duration::from_secs(2)).await;
    }
}

pub async fn queue_counter_spinjob(taskman: Weak<Taskman>) {
    loop {
        sleep(Duration::from_secs(2)).await;
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

            // If the task is delayed...
            let queue_size_if_not_delayed = if taskdata
                .delay_processing_until
                .map(|x| x > Utc::now())
                .unwrap_or(false)
            {
                None
            } else {
                let Some(queue_size) = taskman
                    .db
                    .get_queue_size_for_task(taskid)
                    .await
                    .expect("Database died!")
                else {
                    continue;
                };

                if taskdata.in_progress {
                    // It's being worked on. Don't touch it.
                    continue;
                }

                Some(queue_size)
            };

            let response = taskdata
                .task
                .produce_queue_message(queue_size_if_not_delayed, None);

            if taskman
                .bot
                .edit_message_text(
                    taskdata.queue_message_chat_id,
                    taskdata.queue_message_id,
                    response,
                )
                .disable_link_preview(true)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await
                .is_err()
            {
                continue;
            }
        }

        if !got_a_single_task {
            drop(queue);
            drop(taskman);
            notified.await;
        }
        //sleep(Duration::from_secs(5)).await;
    }
}
