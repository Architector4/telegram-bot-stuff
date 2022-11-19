use std::fs;
use teloxide::prelude::*;

use arch_bot_commons::*;

/// Runs a Telegram bot and prints all messages it receives to stdout.
/// This is useful to upload files to the bot to gather their file IDs
/// to use with other bots.
async fn run_bot_to_fetch_file() {
    log::info!("Starting The thingamading...");

    let key = fs::read_to_string("key_debug").expect("Could not load bot key file!");
    let bot = Bot::new(key);

    let handler = Update::filter_message().endpoint(|_bot: Bot, msg: Message| async move {
        println!("{:#?}", msg);
        respond(())
    });

    log::info!("Dispatching the bot to listen and print all messages.");
    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}

fn main() {
    start_everything(run_bot_to_fetch_file());
}
