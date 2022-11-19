use std::fs;
use teloxide::prelude::*;

use arch_bot_commons::*;

async fn lol() {
    log::info!("ASYNC WOOOO");
    let key = fs::read_to_string(match cfg!(debug_assertions) {
        true => "key_debug",
        false => "key",
    })
    .expect("no bot key file!");

    let bot = Bot::new(key);

    let handler = Update::filter_message().endpoint(|bot: Bot, msg: Message| async move {
        bot.send_message(msg.chat.id, "A").await?;
        tokio::time::sleep(tokio::time::Duration::from_secs_f64(3.0)).await;
        bot.send_message(msg.chat.id, "B").await?;
        respond(())
    });
    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}

fn main() {
    start_everything(lol());
}
