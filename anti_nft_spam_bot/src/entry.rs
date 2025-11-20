use std::{fs, sync::Arc};
use teloxide::{dptree::deps, prelude::*};

use crate::{
    actions::remind_about_reviews_spinloop,
    database::Database,
    handlers::{generate_bot_commands, handle_callback_query},
};

/// # Panics
///
/// Panics if there's no key file
pub async fn entry() {
    log::info!("ASYNC WOOOO");
    let key = fs::read_to_string(match cfg!(debug_assertions) {
        true => "key_debug",
        false => "key",
    })
    .expect("Could not load bot key file!");

    let bot = Bot::new(key);

    bot.set_my_commands(generate_bot_commands())
        .await
        .expect("Failed to set bot commands!");

    let database: Arc<Database> = Database::new().await.expect("Failed to create database!");

    // Already imported.
    //if let Err(e) = database.import_from_old_database().await {
    //    log::warn!("Failed to import from old database: {e}");
    //};

    tokio::spawn(remind_about_reviews_spinloop(
        bot.clone(),
        Arc::downgrade(&database),
    ));

    log::info!("Creating the handler...");

    let handler = dptree::entry()
        .branch(Update::filter_message().branch(dptree::endpoint(
            crate::handlers::handle_message_new_or_edit,
        )))
        .branch(Update::filter_edited_message().branch(dptree::endpoint(
            crate::handlers::handle_message_new_or_edit,
        )))
        .branch(Update::filter_callback_query().endpoint(handle_callback_query));

    log::info!("Dispatching the dispatcher!");

    Dispatcher::builder(bot, handler)
        .default_handler(|_| async {})
        .dependencies(deps![database])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}
