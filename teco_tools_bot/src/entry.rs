use std::{fs, sync::Arc};
use teloxide::{dptree::deps, prelude::*, RequestError};

use crate::{
    handlers,
    tasks::taskman::{database::Database, Taskman},
    USE_LOCAL_API,
};

/// # Panics
/// Panics if the bot fails to start lol
pub async fn entry() {
    magick_rust::magick_wand_genesis();

    log::info!("ASYNC WOOOO");
    let key = fs::read_to_string(match cfg!(debug_assertions) {
        true => "key_debug",
        false => "key",
    })
    .expect("Could not load bot key file!");

    let bot = Bot::new(key);

    let bot = if USE_LOCAL_API {
        bot.set_api_url(url::Url::parse("http://127.0.0.1:8081/tbas").unwrap())
    } else {
        bot
    };

    let db = Arc::new(Database::new().await.expect("Could not init the database!"));

    let commands = crate::handlers::commands::Command::generate_bot_commands();
    bot.set_my_commands(commands)
        .await
        .expect("Failed to set bot commands!");

    let taskman = Taskman::new(db, bot.clone()).await;

    log::info!("Creating the handler...");

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handlers::handle_new_message))
        .branch(Update::filter_edited_message().endpoint(handlers::handle_edited_message))
        .endpoint(|| async { Ok::<(), RequestError>(()) }); // bye lol

    log::info!("Dispatching the dispatcher!");

    Dispatcher::builder(bot, handler)
        .dependencies(deps![taskman])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");

    magick_rust::magick_wand_terminus();
}
