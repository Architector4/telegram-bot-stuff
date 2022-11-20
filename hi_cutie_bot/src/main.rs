use rand::{seq::SliceRandom, Rng};
use regex::Regex;
use std::fs;
use teloxide::{
    prelude::*,
    types::{
        InlineQueryResult, InlineQueryResultArticle, InlineQueryResultCachedVoice, InputFile,
        InputMessageContent, InputMessageContentText,
    },
};

use arch_bot_commons::*;
use once_cell::sync::Lazy;
use tokio::time::{sleep, Duration};

fn gen_password() -> String {
    static ALPHABET: Lazy<Vec<char>> =
        Lazy::new(|| (b'a'..=b'z').map(|c| c as char).collect::<Vec<_>>());
    static RESPONSES: &[&str] = &[
        " hi",
        " hhi",
        " hhhi",
        " hh",
        " STOP",
        " omg",
        " aaaaa",
        " aaaaaaaaaaaaaaaa",
    ];
    let mut rng = rand::thread_rng();
    let length = rng.gen_range(8..69);

    let mut password = (0..length)
        .map(|_| ALPHABET.choose(&mut rng).unwrap())
        .collect::<String>();

    if rng.gen::<f64>() < 0.45 {
        let response = RESPONSES.choose(&mut rng).unwrap();
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

    static REGEXMOMENT: Lazy<regex::Regex> = Lazy::new(|| Regex::new("hi+,? +cutie.*").unwrap());
    static REGEXMOMENT_HERBERT: Lazy<regex::Regex> =
        Lazy::new(|| Regex::new("hi+,? +herbert.*").unwrap());
    // Hardcoded file ID
    static MOW_FILE_ID: Lazy<String> = Lazy::new(|| {
        String::from("AwACAgIAAxkDAAIDJ2N3tP7v2E5m3XaD33yBSDRasamhAAI1IQACUYzBSyvEssWVfZM2KwQ")
    });

    log::info!("Creating the handler...");

    let handler = dptree::entry()
        .branch(
            Update::filter_inline_query().endpoint(|bot: Bot, q: InlineQuery| async move {
                bot.answer_inline_query(&q.id, {
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
                        results.push(InlineQueryResult::CachedVoice(
                            InlineQueryResultCachedVoice::new(
                                "mow",
                                MOW_FILE_ID.to_owned(),
                                "meow",
                            ),
                        ));
                    }
                    results
                })
                .await?;
                respond(())
            }),
        )
        .branch(Update::filter_message().endpoint(
            |bot: Bot, username: String, msg: Message| async move {
                static HELP: &str =
                    "(this bot answers to \"hi, cutie!\" messages in DMs and group chats)";
                if let Some(text) = msg.text() {
                    if msg.chat.is_private() && text == "/start" {
                        bot.send_message(msg.chat.id, HELP)
                            .reply_to_message_id(msg.id)
                            .await?;
                    } else {
                        let text = text.to_lowercase().replace(username.as_str(), "");
                        let text = text.trim();

                        if REGEXMOMENT.is_match(text) {
                            bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
                                .await?;

                            sleep(Duration::from_secs_f64(rand::random::<f64>() * 2.0 + 5.5)).await;

                            bot.send_message(msg.chat.id, gen_password())
                                .reply_to_message_id(msg.id)
                                .await?;
                        } else if REGEXMOMENT_HERBERT.is_match(text) {
                            bot.send_voice(msg.chat.id, InputFile::file_id(MOW_FILE_ID.to_owned()))
                                .reply_to_message_id(msg.id)
                                .await?;
                        }
                    }
                }
                respond(())
            },
        ));

    log::info!("Dispatching the dispatcher!");

    let username = String::from("@") + bot.get_me().await.unwrap().username();

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![username])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    log::info!("it appears we have been bonked.");
}

fn main() {
    start_everything(lol());
}
