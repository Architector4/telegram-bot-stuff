//! This create houses common for me functions, because some things
//! are just boilerplate and aaAAAAAAAAA

use std::future::Future;

use teloxide::{
    prelude::*,
    types::{Chat, MessageEntity, User},
};

pub mod user_resolving;

// this is cursed lol
//pub async fn make_interruptible(f: impl Future) {
//    use tokio::select;
//    select! {
//        _ = f => (),
//        _ = tokio::signal::ctrl_c() => (),
//    }
//}

/// Initialize logging and start the `closure` in an async runtime.
/// Logging is enabled by default on level `info` unless overridden
/// by environment variable `RUST_LOG`. This uses the crate
/// [pretty_env_logger][] internally, see its documentation for more details.
///
/// [pretty_env_logger]: https://docs.rs/pretty_env_logger
pub fn start_everything(closure: impl Future<Output = ()>) {
    let log_level = std::env::var_os("RUST_LOG")
        .unwrap_or_else(|| std::ffi::OsString::from("info"))
        .into_string()
        .unwrap_or_else(|_| String::from("info"));

    let running_as_systemd_service = std::env::var_os("JOURNAL_STREAM").is_some();

    let mut builder = match running_as_systemd_service {
        true => pretty_env_logger::formatted_builder(),
        false => pretty_env_logger::formatted_timed_builder(),
    };

    builder.parse_filters(&log_level);

    if builder.try_init().is_err() {
        log::error!("Tried to init logger twice!");
    }

    log::info!("hi");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(closure);
}

/// Find out if a user of this ID is an admin of the specified chat of that ID.
/// If so, returns the `ChatMember` object describing their permissions,
/// otherwise `None`.
pub async fn get_admin_of(
    bot: &Bot,
    user: UserId,
    chat: ChatId,
) -> Result<Option<teloxide::types::ChatMember>, teloxide::RequestError> {
    Ok(bot
        .get_chat_administrators(chat)
        .await?
        .into_iter()
        .find(|x| x.user.id == user))
}

/// Create a string that can be used in a message to refer to
/// a particular user. Guaranteed to tag them.
pub fn print_user(user: &User) -> (String, Option<MessageEntity>) {
    match user.username {
        Some(ref username) => {
            let mut output = String::with_capacity(username.len() + 1);
            output.push('@');
            output.push_str(username.as_str());
            (output, None)
        }
        None => {
            let first_name = user.first_name.clone();
            let last_name = &user.last_name;
            let full_name = match last_name {
                None => first_name,
                Some(last_name) => first_name + " " + last_name,
            };
            let len = full_name.len();
            (
                full_name,
                Some(MessageEntity::text_mention(user.to_owned(), 0, len)),
            )
        }
    }
}

/// Create a string that can be used in a message to refer to
/// a chat or a channel. Will be clickable only for public ones.
pub fn print_chat(chat: &Chat) -> Option<String> {
    match chat.username() {
        Some(x) => Some(format!("@{}", x)),
        None => chat.title().map(String::from),
    }
}

/// Create a string that can be used in a message to refer to
/// the sender of this message. Guaranteed to tag them, unless they
/// are posting anonymously or as a channel.
pub fn print_sender(message: &Message) -> (String, Option<MessageEntity>) {
    match message.from() {
        Some(user) => print_user(user),
        None => (
            {
                static ANONYMOUS_ADMIN: &str = "Anonymous admin";
                match message.author_signature() {
                    None => match message.sender_chat() {
                        None => ANONYMOUS_ADMIN.into(),
                        Some(chat) => {
                            if chat.id == message.chat.id {
                                "Some channel".into()
                            } else {
                                print_chat(chat).unwrap_or_else(|| ANONYMOUS_ADMIN.into())
                            }
                        }
                    },
                    Some(sig) => String::from(sig),
                }
            },
            None,
        ),
    }
}
