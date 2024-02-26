use std::{fs, sync::Arc};
use teloxide::{dptree::deps, prelude::*};

use crate::domains::database::Database;

pub async fn entry() {
    log::info!("ASYNC WOOOO");
    let key = fs::read_to_string(match cfg!(debug_assertions) {
        true => "key_debug",
        false => "key",
    })
    .expect("Could not load bot key file!");

    let bot = Bot::new(key);

    let db = match Database::new().await {
        Ok(db) => Arc::new(db),
        Err(e) => panic!("Failure creating the database! {:?}", e),
    };

    log::info!("Creating the handler...");

    let handler =
        Update::filter_message().branch(dptree::endpoint(crate::handlers::handle_message));

    log::info!("Dispatching the dispatcher!");

    Dispatcher::builder(bot, handler)
        .dependencies(deps![db])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}
