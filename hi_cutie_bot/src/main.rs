use rand::{seq::IndexedRandom, Rng};
use regex::Regex;
use std::{fs, sync::LazyLock};
use teloxide::{
    prelude::*,
    sugar::request::RequestReplyExt,
    types::{
        InlineQueryResult, InlineQueryResultArticle, InlineQueryResultVoice, InputFile,
        InputMessageContent, InputMessageContentText, Me,
    },
};
use url::Url;

use arch_bot_commons::*;
use tokio::time::{sleep, Duration};

fn gen_password() -> String {
    static RESPONSES: &[&str] = &[
        " hi",
        " hhi",
        " hhhi",
        " hh",
        " STOP",
        " omg",
        " aaaaa",
        " aaaaaaaaaaaaaaaa",
        " pls 🥺",
    ];
    let mut rng = rand::rng();
    let length = rng.random_range(8..69);

    let mut password = (0..length)
        .map(|_| rng.random_range('a'..='z'))
        .collect::<String>();

    if rng.random_range(0.0..1.0) < 0.45 {
        let response = RESPONSES
            .choose(&mut rng)
            .expect("There is always multiple possible responses");
        password.push_str(response);
    }

    password
}

async fn lol() {
    log::info!("ASYNC WOOOO");
    let key = fs::read_to_string(match cfg!(debug_assertions) {
        true => "key_debug",
        false => "key",
    })
    .expect("Could not load bot key file!");

    let bot = Bot::new(key);

    static REGEXMOMENT: LazyLock<regex::Regex> =
        LazyLock::new(|| Regex::new("(hi|hey)+,? +cutie.*").expect("Regex will always be valid"));
    static REGEXMOMENT_HERBERT: LazyLock<regex::Regex> =
        LazyLock::new(|| Regex::new("hi+,? +herbert.*").expect("Regex will always be valid"));
    // Hardcoded file ID
    static MOW_URL: LazyLock<Url> = LazyLock::new(|| {
        Url::parse("https://architector4.tilde.team/stuff/mow.ogg")
            .expect("URL will always be valid")
    });

    log::info!("Creating the handler...");

    let handler = dptree::entry()
        .branch(
            Update::filter_inline_query().endpoint(|bot: Bot, q: InlineQuery| async move {
                bot.answer_inline_query(q.id, {
                    let mut results = (0..10)
                        .map(|i| {
                            let p = gen_password();
                            InlineQueryResult::Article(InlineQueryResultArticle::new(
                                i.to_string(),
                                p.clone(),
                                InputMessageContent::Text(InputMessageContentText::new(p)),
                            ))
                        })
                        .collect::<Vec<_>>();
                    if REGEXMOMENT_HERBERT.is_match(&q.query) {
                        results.push(InlineQueryResult::Voice(InlineQueryResultVoice::new(
                            "mow",
                            MOW_URL.to_owned(),
                            "meow",
                        )));
                    }
                    results
                })
                .await?;
                respond(())
            }),
        )
        .branch(
            Update::filter_message().endpoint(|bot: Bot, me: Me, msg: Message| async move {
                static HELP: &str =
                    "(this bot answers to \"hi, cutie!\" messages in DMs and group chats)";
                if let Some(text) = msg.text() {
                    if msg.chat.is_private() && text == "/start" {
                        bot.send_message(msg.chat.id, HELP).reply_to(msg.id).await?;
                    } else {
                        let username = String::from("@") + me.username();
                        let text = text.to_lowercase().replace(username.as_str(), "");
                        let text = text.trim();

                        if REGEXMOMENT.is_match(text) {
                            bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
                                .await?;

                            sleep(Duration::from_secs_f64(rand::random::<f64>() * 3.0 + 2.0)).await;

                            bot.send_message(msg.chat.id, gen_password())
                                .reply_to(msg.id)
                                .await?;
                        } else if REGEXMOMENT_HERBERT.is_match(text) {
                            bot.send_voice(msg.chat.id, InputFile::url(MOW_URL.to_owned()))
                                .reply_to(msg.id)
                                .await?;
                        }
                    }
                }
                respond(())
            }),
        );

    log::info!("Dispatching the dispatcher!");

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}

fn main() {
    start_everything(lol());
}
