mod parser;

use std::{
    io::{Error, ErrorKind},
    path::Path,
    sync::Arc,
};

use notify::{RecursiveMode, Watcher};
use teloxide::Bot;

use crate::types::IsSpam;
use parser::Line;

static LIST_FILE: &str = "spam_website_list.txt";

pub async fn watch_list(bot: Bot, db_arc: Arc<super::Database>) {
    // First ingest ASAP...
    let _ = ingest_list_to_database(&bot, &db_arc).await;

    let mut receiver = db_arc.drop_watch.0.subscribe();
    let database = Arc::downgrade(&db_arc);
    drop(db_arc);

    let update_notify = Arc::new(tokio::sync::Notify::new());
    let update_notify_watcher_clone = update_notify.clone();

    let mut watcher =
        notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
            let event = event.unwrap();
            let k = event.kind;
            if k.is_create() || k.is_modify() || k.is_other() {
                for path in &event.paths {
                    if path.ends_with(LIST_FILE) {
                        update_notify_watcher_clone.notify_waiters();
                        break;
                    }
                }
            }
        })
        .unwrap();

    watcher
        .watch(Path::new("."), RecursiveMode::NonRecursive)
        .unwrap();

    loop {
        tokio::select! {
            () = update_notify.notified() => {
                // Notified of a change happening. Check the file.
                log::debug!("Notified of a file change!");
                let Some(database) = database.upgrade() else {
                    // This means the database was dropped.
                    break;
                };

                let _ = ingest_list_to_database(&bot, &database).await;

            },
            e = receiver.changed() => {
                // This means that the database was dropped.
                let Err(_e) = e else {
                    // Make sure this isn't someone sending a message.
                    // That shouldn't be done.
                    unreachable!();
                };

                break;
            }
        };
    }
}

async fn ingest_list_to_database(bot: &Bot, database: &super::Database) -> std::io::Result<()> {
    log::info!("Ingesting list to database...");
    use std::{fs::File, io::BufReader};
    use teloxide::requests::Requester;

    let file = File::open(LIST_FILE)?;
    let reader = BufReader::new(file);

    let mut parser = parser::Parser::new(reader);

    let mut error_counter: u8 = 0;

    let mut cleaned_previous_entries = false;

    while let Some(line) = parser.next_line() {
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => continue,
            Err(e) => {
                let mut error_message = format!("Error while parsing scam website list:\n{e}");
                log::warn!("{error_message}");
                if error_counter < 3 {
                    error_counter += 1;

                    if error_counter >= 3 {
                        error_message += "\n\nSuppressing further errors.";
                    }

                    // Don't care if this fails. What can we do, log it?
                    // The error above will show up in the log anyway lol
                    let _ = bot
                        .send_message(crate::CONTROL_CHAT_ID, error_message)
                        .await;
                }
                continue;
            }
        };

        // NOW that we finally have a line...

        let mut database_result = Ok(());

        if !cleaned_previous_entries {
            log::info!("Cleaning previous entries...");
            database_result = database.clean_all_from_spam_list().await;
            cleaned_previous_entries = true;
        }

        if database_result.is_ok() {
            database_result = match line {
                Line::Url(url) => database.add_url(&url, IsSpam::Yes, true, false).await,
                Line::Domain {
                    domain,
                    example_url,
                } => {
                    database
                        .add_domain(&domain, Some(&example_url), IsSpam::Yes, true, false)
                        .await
                }
            };
        }

        if let Err(e) = database_result {
            let error_message = format!("Failed to ingest spam list to database:\n{e}");
            log::warn!("{error_message}");
            // Don't care if this fails. What can we do, log it?
            // The error above will show up in the log anyway lol
            let _ = bot
                .send_message(crate::CONTROL_CHAT_ID, error_message)
                .await;
            return Err(Error::new(ErrorKind::BrokenPipe, e));
        }
    }

    log::info!("Ingested list successfully.");

    Ok(())
}
