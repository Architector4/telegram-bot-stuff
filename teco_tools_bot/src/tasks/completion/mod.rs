mod media_processing;
use arch_bot_commons::{teloxide_retry, useful_methods::*};
use teloxide::{
    payloads::{SendMessageSetters, SendPhotoSetters, SendStickerSetters},
    requests::Requester,
    types::InputFile,
    Bot, RequestError,
};

use super::{taskman::database::TaskDatabaseInfo, ImageFormat, Task};

impl Task {
    pub async fn complete_task(
        &self,
        bot: &Bot,
        data: &TaskDatabaseInfo,
    ) -> Result<(), RequestError> {
        macro_rules! respond {
            ($text:expr) => {
                bot.send_message(data.message.chat.id, $text)
                    .reply_to_message_id(data.message.id)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
            };
        }

        macro_rules! goodbye {
            ($text:expr) => {{
                respond!($text);
                return Ok(());
            }};
        }

        #[allow(unused_macros)]
        macro_rules! unfail_any {
            ($thing:expr) => {{
                match $thing {
                    Ok(woot) => woot,
                    Err(e) => {
                        return Err(RequestError::Api(ApiError::Unknown(format!(
                            "NOT TELEGRAM ERROR BUT: {:#?}",
                            e
                        ))))
                    }
                };
            }};
        }

        match self {
            Task::Amogus { amogus } => {
                let sign = amogus.signum();
                let count = amogus.unsigned_abs();

                let mut response;

                // Those are maximum amogus counts that can fit in
                // a 4096 characters long message lol
                //let response_str = if sign == -1 && count > 255 || sign == 1 && count > 585 {
                // Let's be more modest than that.
                let response_str = if count > 16 {
                    if sign == -1 {
                        "<b>TOO MUCH NEGATIVE AMOGUS</b>"
                    } else {
                        "<b>TOO MUCH AMOGUS</b>"
                    }
                } else {
                    response = String::from("<b>");

                    let response_single = match sign {
                        -1 => "NEGATIVE AMOGUS ",
                        1 => "AMOGUS ",
                        0 => return Ok(()),
                        _ => unreachable!(),
                    };

                    let length: usize = response_single.len() + 4; // for "</b>"
                    response.reserve_exact(length);

                    for _ in 0..count {
                        response.push_str(response_single);
                    }

                    response.push_str("</b>");

                    response.as_str()
                };

                goodbye!(response_str);
            }
            Task::ImageResize {
                new_dimensions,
                percentage: _,
                mut format,
                resize_type,
            } => {
                let photo = data.message.get_media_info();
                let photo = match photo {
                    Some(photo) => {
                        if !photo.is_image() {
                            goodbye!(
                                "Error: can't work with video nor animated nor video stickers."
                            );
                        }
                        if photo.file.size > 20 * 1000 * 1000 {
                            goodbye!("Error: media is too large.");
                        }
                        photo
                    }
                    None => goodbye!("Error: can't find an image."),
                };

                if format == ImageFormat::Preserve {
                    if photo.is_sticker {
                        format = ImageFormat::Webp;
                    } else {
                        format = ImageFormat::Jpeg;
                    }
                }

                let should_be_sticker = format.supports_alpha_transparency();

                let mut img_data: Vec<u8> = Vec::new();

                bot.download_file_to_vec(photo.file, &mut img_data).await?;

                let dimensions = (
                    new_dimensions.0.get() as usize,
                    new_dimensions.1.get() as usize,
                );
                let resize_type = *resize_type;

                let woot = tokio::task::spawn_blocking(move || {
                    media_processing::resize_image(
                        img_data,
                        dimensions.0,
                        dimensions.1,
                        resize_type,
                        format,
                    )
                })
                .await
                .expect("Worker died!");
                let Ok(img_data) = woot else {
                    let wat = woot.unwrap_err();
                    log::error!("{}", wat);

                    goodbye!("Error: failed to parse the image");
                };

                teloxide_retry!({
                    let send = img_data.clone();
                    if should_be_sticker {
                        bot.send_sticker(data.message.chat.id, InputFile::memory(send))
                            .reply_to_message_id(data.message.id.0)
                            .await
                    } else {
                        bot.send_photo(data.message.chat.id, InputFile::memory(send))
                            .reply_to_message_id(data.message.id)
                            .await
                    }
                })?;
                Ok(())
            }
        }
    }
}
