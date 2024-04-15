use std::{fs, sync::Arc};
use teloxide::{dptree::deps, prelude::*};

use crate::{database::Database, handlers::reviews::parse_callback_query};

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

    let db: Arc<Database> = Database::new(bot.clone()).await.unwrap();

    log::info!("Creating the handler...");

    let handler = dptree::entry()
        .branch(Update::filter_message().branch(dptree::endpoint(crate::handlers::handle_message)))
        .branch(Update::filter_callback_query().endpoint(parse_callback_query));

    log::info!("Dispatching the dispatcher!");

    Dispatcher::builder(bot, handler)
        .dependencies(deps![db])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}
