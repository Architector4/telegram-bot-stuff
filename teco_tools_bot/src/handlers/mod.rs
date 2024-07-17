pub mod commands;
use arch_bot_commons::{teloxide_retry, useful_methods::*};
use chrono::Utc;

use std::sync::Arc;

use teloxide::{
    payloads::{EditMessageTextSetters, SendMessageSetters},
    requests::Requester,
    types::{Me, Message},
    Bot, RequestError,
};

use crate::tasks::{parsing::TaskError, taskman::Taskman, Task};

pub async fn parse_command_into_task(
    taskman: &Taskman,
    bot: &Bot,
    bot_me: &Me,
    message: &Message,
) -> Result<Result<Task, TaskError>, RequestError> {
    let Some(task) = Task::parse_task(taskman, bot, bot_me, message) else {
        return Ok(Err(TaskError::Error(String::new())));
    };

    task.await
}

pub async fn handle_new_message(
    bot: Bot,
    me: Me,
    message: Message,
    taskman: Arc<Taskman>,
) -> Result<(), RequestError> {
    let sender_id = message.from().map(|from| from.id);
    // Bot ignores messages made by itself.
    if sender_id == Some(me.id) {
        return Ok(());
    }

    let task = match parse_command_into_task(&taskman, &bot, &me, &message).await? {
        Ok(t) => t,
        Err(e) => {
            if !e.is_empty() {
                bot.send_message(message.chat.id, e.cancel_to_error().to_string())
                    .disable_web_page_preview(true)
                    .reply_to_message_id(message.id)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
            }
            return Ok(());
        }
    };

    // We got a task. Reply about it and push it to queue.

    let premium = if let Some(sender_id) = sender_id {
        taskman
            .db
            .is_user_premium(sender_id)
            .await
            .expect("Database died!")
    } else {
        false
    };

    let overquota = taskman
        .db
        .user_has_too_much_tasks(sender_id)
        .await
        .expect("Database died!");

    if overquota {
        bot.send_message(
            message.chat.id,
            if sender_id.is_some() {
                concat!(
                    "Sorry, but you have too many tasks queued up at the moment. ",
                    "Please try again later."
                )
            } else {
                concat!(
                    "Sorry, but anonymous users have too many tasks queued up at the moment. ",
                    "Please try again later."
                )
            },
        )
        .reply_to_message_id(message.id)
        .await?;

        return Ok(());
    }

    // Check if this chat has a slow mode.
    // If we're here, we've JUST sent the queue response message,
    // This means that the next message we can send will only be after
    // the slow mode delay.

    // Get full chat info, if we can and if it isn't a private chat.
    // (private chats don't have slow mode lol)
    let chat_real = if message.chat.is_private() {
        None
    } else {
        bot.get_chat(message.chat.id).await.ok()
    };

    // If we got it, get its slow mode delay.
    let slow_mode_delay = chat_real.and_then(|x| x.slow_mode_delay());
    // Convert it into a duration, and add 3 extra seconds.
    // The additional seconds should end up with the bot prioritizing new task request messages
    // over completing tasks.
    let slow_mode_delay =
        slow_mode_delay.map(|x| chrono::Duration::seconds(x.saturating_add(3).into()));
    // Convert that into a datetime when that duration expires.
    let delay_processing_until = slow_mode_delay.map(|x| chrono::Utc::now() + x);

    let queue_size = taskman
        .db
        .get_queue_size(premium)
        .await
        .expect("Database died!");

    let response =
        task.produce_queue_message(delay_processing_until.is_none().then_some(queue_size), None);

    let queue_response_message = teloxide_retry!(
        bot.send_message(message.chat.id, &response)
            .reply_to_message_id(message.id)
            .disable_web_page_preview(true)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await
    )?;

    if message.media_group_id().is_some() {
        bot.send_message(
            message.chat.id,
            concat!(
                "<b>Important:</b> this bot does not support album uploads. ",
                "Please reply with the command to each media separately."
            ),
        )
        .reply_to_message_id(message.id)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;
    }

    taskman
        .add_task(
            sender_id,
            task,
            &message,
            &queue_response_message,
            delay_processing_until,
        )
        .await
        .expect("Database died!");

    Ok(())
}

pub async fn handle_edited_message(
    bot: Bot,
    me: Me,
    message: Message,
    taskman: Arc<Taskman>,
) -> Result<(), RequestError> {
    let Some(taskdata) = taskman
        .db
        .get_task_by_request_message(&message)
        .await
        .expect("Database died!")
    else {
        return Ok(());
    };

    if let (Some(edit_response_chat_id), Some(edit_response_message_id)) = (
        taskdata.edit_response_chat_id,
        taskdata.edit_response_message_id,
    ) {
        let _ = bot
            .delete_message(edit_response_chat_id, edit_response_message_id)
            .await;
    }

    if taskman
        .db
        .get_queue_size_for_task(taskdata.taskid)
        .await
        .expect("Database died!")
        == Some(0)
    {
        // Task is currently being run.
        let edit_response = bot
            .send_message(
                message.chat.id,
                concat!(
                    "Sorry, but the task is currently being run. ",
                    "Canceling or editing parameters is not possible at the moment."
                ),
            )
            .reply_to_message_id(message.id)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?;

        taskman
            .db
            .task_edit_response(taskdata.taskid, &edit_response)
            .await
            .expect("Database died!");

        return Ok(());
    }

    let task = match parse_command_into_task(&taskman, &bot, &me, &message).await? {
        Ok(t) => t,
        Err(e) => {
            let cancelling = e.is_cancel() || message.text_full().unwrap().starts_with("/cancel");
            if cancelling {
                if let TaskError::Cancel(_) = e {
                    bot.send_message(message.chat.id, e.to_string())
                        .reply_to_message_id(message.id)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                }

                // Edit response message is deleted, and request message is bogus.
                // Delete queue message and from the database lol
                let _ = bot
                    .delete_message(taskdata.queue_message_chat_id, taskdata.queue_message_id)
                    .await;

                taskman
                    .db
                    .delete_task(taskdata.taskid)
                    .await
                    .expect("Database died!");
            } else {
                let mut e_txt = e.to_string();
                if e.is_empty() {
                    e_txt.push_str("Failed to parse the command as a task.");
                }
                e_txt.push_str(concat!(
                    "\n\nWill use the previous parameters for the task.\n",
                    "If you wish to cancel the task, edit your message to say <code>/cancel</code>.\n",
                    "(Telegram bots can't see message deletion events, by the way)",
                ));
                let edit_response = bot
                    .send_message(message.chat.id, e_txt)
                    .reply_to_message_id(message.id)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;

                taskman
                    .db
                    .task_edit_response(taskdata.taskid, &edit_response)
                    .await
                    .expect("Database died!");
            }

            return Ok(());
        }
    };

    taskman
        .db
        .edit_task(taskdata.taskid, &task, &message)
        .await
        .expect("Database died!");

    let queue_size = taskman
        .db
        .get_queue_size_for_task(taskdata.taskid)
        .await
        .expect("Database died!")
        .unwrap();

    let is_delayed = taskdata
        .delay_processing_until
        .map(|x| x > Utc::now())
        .unwrap_or(false);

    let response = task.produce_queue_message(is_delayed.then_some(queue_size), None);

    let _ = bot
        .edit_message_text(
            taskdata.queue_message_chat_id,
            taskdata.queue_message_id,
            response,
        )
        .disable_web_page_preview(true)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await;

    Ok(())
}
