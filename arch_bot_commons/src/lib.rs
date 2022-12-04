//! This create houses common for me functions, because some things
//! are just boilerplate and aaAAAAAAAAA

use std::future::Future;

use teloxide::prelude::*;

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
