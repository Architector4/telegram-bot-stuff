mod media_processing;
use arch_bot_commons::{teloxide_retry, useful_methods::*};
use crossbeam_channel::Sender;
use teloxide::{
    payloads::{
        SendAnimationSetters, SendMessageSetters, SendPhotoSetters, SendStickerSetters,
        SendVideoSetters,
    },
    requests::Requester,
    types::InputFile,
    Bot, RequestError,
};

use super::{taskman::database::TaskDatabaseInfo, ImageFormat, Task};

impl Task {
    pub async fn complete_task(
        &self,
        status_report: Sender<String>,
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
                rotation,
                percentage: _,
                format: _,
                resize_type,
            }
            | Task::VideoResize {
                new_dimensions,
                rotation,
                percentage: _,
                resize_type,
            } => {
                let media = data.message.get_media_info();
                let media = match media {
                    Some(media) => {
                        if media.is_vector_sticker {
                            goodbye!("Error: can't work with animated stickers.");
                        }
                        if media.file.size > 20 * 1000 * 1000 {
                            goodbye!("Error: media is too large.");
                        }
                        media
                    }
                    None => goodbye!("Error: can't find the media.."),
                };
                let format = if let Task::ImageResize { format, .. } = self {
                    if media.is_video {
                        goodbye!("Error: expected an image to resize, but found a video instead.");
                    }
                    if *format == ImageFormat::Preserve {
                        if media.is_sticker {
                            ImageFormat::Webp
                        } else {
                            ImageFormat::Jpeg
                        }
                    } else {
                        *format
                    }
                } else {
                    if !media.is_video {
                        goodbye!("Error: expected a video to resize, but found an image instead.");
                    }
                    ImageFormat::Preserve
                };

                let should_be_sticker = !media.is_video && format.supports_alpha_transparency();

                let mut media_data: Vec<u8> = Vec::new();

                bot.download_file_to_vec(media.file, &mut media_data)
                    .await?;

                let dimensions = (
                    new_dimensions.0.get() as usize,
                    new_dimensions.1.get() as usize,
                );
                let resize_type = *resize_type;
                let rotation = *rotation;

                let woot = tokio::task::spawn_blocking(move || {
                    if media.is_video {
                        media_processing::resize_video(
                            status_report,
                            media_data,
                            dimensions.0,
                            dimensions.1,
                            rotation,
                            resize_type,
                        )
                    } else {
                        media_processing::resize_image(
                            &media_data,
                            dimensions.0,
                            dimensions.1,
                            rotation,
                            resize_type,
                            format,
                        )
                        .map_err(|e| e.to_string())
                    }
                })
                .await
                .expect("Worker died!");
                let Ok(media_data) = woot else {
                    let wat = woot.unwrap_err();
                    log::error!("{}", wat);

                    goodbye!("Error: failed to process the media");
                };

                if media_data.is_empty() {
                    goodbye!(
                        "Error: failed to process the media; got empty file as a result. Sorry!"
                    );
                }

                teloxide_retry!({
                    let send = media_data.clone();
                    if media.is_video {
                        if media.is_gif || media.is_sticker {
                            bot.send_animation(
                                data.message.chat.id,
                                InputFile::memory(send).file_name("amogus.mp4"),
                            )
                            .reply_to_message_id(data.message.id)
                            .await
                        } else {
                            bot.send_video(data.message.chat.id, InputFile::memory(send))
                                .reply_to_message_id(data.message.id)
                                .await
                        }
                    } else if should_be_sticker {
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
