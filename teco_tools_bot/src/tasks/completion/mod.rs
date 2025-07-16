pub mod media_processing;
use arch_bot_commons::{teloxide_retry, useful_methods::*};
use html_escape::encode_text;
use media_processing::whisper;
use teloxide::{
    payloads::{SendAnimationSetters, SendPhotoSetters, SendVideoSetters},
    requests::Requester,
    sugar::request::RequestReplyExt,
    types::InputFile,
    ApiError, Bot, RequestError,
};
use tokio::sync::watch::Sender;

use crate::{
    tasks::{ResizeCurve, ResizeType, VideoTypePreference},
    MAX_DOWNLOAD_SIZE_MEGABYTES, MAX_UPLOAD_SIZE_MEGABYTES,
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
                bot.archsendmsg(data.message.chat.id, $text, data.message.id)
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

        // Little handler for the downloading.
        macro_rules! unerror_download {
            ($download: expr) => {{
                let result = $download;
                if let Err(RequestError::Api(ApiError::Unknown(text))) = &result {
                    if text.contains("file is temporarily unavailable") {
                        goodbye!(concat!(
                            "Error: the media file is unavailable for the bot. ",
                            "This is likely a Telegram server issue. ",
                            "Try reuploading and performing the command again."
                        ));
                    }
                };
                if let Err(RequestError::Network(_)) = &result {
                    goodbye!(concat!(
                        "Error: a networking error while downloading the file. ",
                        "This is likely a Telegram server issue. ",
                        "Try waiting some time, or reuploading the media, ",
                        "and performing the command again."
                    ));
                };
                result?
            }};
        }

        match self {
            Task::Amogus { amogus } => {
                let sign = amogus.signum();
                let count = amogus.unsigned_abs();

                let mut response;

                let response_str = if count > 16 {
                    if sign == -1 {
                        "<b>TOO MUCH ANTIMOGUS</b>"
                    } else {
                        "<b>TOO MUCH AMOGUS</b>"
                    }
                } else {
                    response = String::from("<b>");

                    let response_single = match sign {
                        -1 => "ANTIMOGUS ",
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
                quality,
            }
            | Task::VideoResize {
                new_dimensions,
                rotation,
                percentage: _,
                resize_type,
                vibrato_hz: _,
                vibrato_depth: _,
                resize_curve: _,
                type_pref: _,
                quality,
            } => {
                let media = data.message.get_media_info();
                let media = match media {
                    Some(media) => {
                        if !media.is_raster() {
                            goodbye!(
                                "Error: can't work with animated stickers nor voice messages."
                            );
                        }
                        if media.file.size > MAX_DOWNLOAD_SIZE_MEGABYTES * 1000 * 1000 {
                            goodbye!(format!(
                                "Error: media is too large. The limit is {MAX_DOWNLOAD_SIZE_MEGABYTES}MB."
                            )
                            .as_str());
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

                let mut media_data: Vec<u8> = Vec::new();

                let input_dimensions = (media.width, media.height);

                let dimensions = (new_dimensions.0 as isize, new_dimensions.1 as isize);
                let mut resize_type = resize_type.clone();
                let rotation = *rotation;
                let quality = *quality;

                let (vibrato_hz, vibrato_depth, resize_curve) = if let Task::VideoResize {
                    vibrato_hz,
                    vibrato_depth,
                    resize_curve,
                    ..
                } = self
                {
                    (*vibrato_hz, *vibrato_depth, *resize_curve)
                } else {
                    (7.0, 0.0, ResizeCurve::default())
                };

                let should_be_gif = if let Task::VideoResize { type_pref, .. } = self {
                    match type_pref {
                        VideoTypePreference::Preserve => media.is_gif || media.is_sticker,
                        VideoTypePreference::Gif => true,
                        VideoTypePreference::Video => false,
                    }
                } else {
                    // Not a video lol
                    false
                };

                let (should_be_spoilered, caption) =
                    if let ResizeType::ToSpoileredMedia { caption } = &mut resize_type {
                        (true, std::mem::take(caption))
                    } else {
                        (false, String::new())
                    };

                let status_report_for_processing = status_report.clone();

                // Variable just to hold the temporary file and drop it later.
                let mut file = None;

                let _ = status_report.send("Downloading media...".to_string());
                let woot = if media.is_video {
                    let download =
                        unerror_download!(bot.download_file_to_temp_or_directly(media.file).await);
                    let path = download.0;
                    file = download.1;
                    tokio::task::spawn_blocking(move || {
                        media_processing::resize_video(
                            status_report_for_processing,
                            &path,
                            dimensions,
                            rotation,
                            resize_type,
                            should_be_gif,
                            vibrato_hz,
                            vibrato_depth,
                            input_dimensions,
                            resize_curve,
                            quality,
                        )
                    })
                } else {
                    let download_result =
                        bot.download_file_to_vec(media.file, &mut media_data).await;
                    unerror_download!(download_result);

                    tokio::task::spawn_blocking(move || {
                        media_processing::resize_image(
                            &media_data,
                            dimensions.0,
                            dimensions.1,
                            rotation,
                            resize_type,
                            format,
                            None,
                            false,
                            quality,
                        )
                        .map_err(|e| e.to_string())
                    })
                }
                .await
                .expect("Worker died!");

                drop(file);

                let media_data = match woot {
                    Ok(m) => m,
                    Err(e) => {
                        log::error!("Error when resizing media: {e}");
                        goodbye!("Error: failed to process the media");
                    }
                };

                if media_data.is_empty() {
                    goodbye!(
                        "Error: failed to process the media; got empty file as a result. Sorry!"
                    );
                }

                if media_data.len() > MAX_UPLOAD_SIZE_MEGABYTES as usize * 1000 * 1000 {
                    goodbye!(format!(
                        "Error: the resulting media is too big ({:.3}MB, max is {}MB). Sorry!",
                        media_data.len() as f64 / 1000.0 / 100.00,
                        MAX_UPLOAD_SIZE_MEGABYTES
                    )
                    .as_str());
                }

                let should_be_sticker = !media.is_video && format.supports_alpha_transparency();

                let _ = status_report.send("Uploading result...".to_string());

                teloxide_retry!({
                    let send = media_data.clone();
                    let result = if media.is_video {
                        if should_be_gif {
                            // Sending as an "animation" requires that the file has a filename, else
                            // it somehow ends up being a file document instead.
                            bot.send_animation(
                                data.message.chat.id,
                                InputFile::memory(send).file_name("amogus.mp4"),
                            )
                            .reply_to(data.message.id)
                            .caption(&caption)
                            .has_spoiler(should_be_spoilered)
                            .await
                        } else {
                            bot.send_video(data.message.chat.id, InputFile::memory(send))
                                .reply_to(data.message.id)
                                .caption(&caption)
                                .has_spoiler(should_be_spoilered)
                                .await
                        }
                    } else if should_be_sticker {
                        bot.send_sticker(data.message.chat.id, InputFile::memory(send))
                            .reply_to(data.message.id)
                            .await
                    } else {
                        bot.send_photo(data.message.chat.id, InputFile::memory(send))
                            .reply_to(data.message.id)
                            .caption(&caption)
                            .has_spoiler(should_be_spoilered)
                            .await
                    };

                    match &result {
                        Err(RequestError::Api(teloxide::ApiError::RequestEntityTooLarge)) => {
                            goodbye!(format!(
                            "Error: the resulting media is too big ({:.3}MB, max is {}MB). Sorry!",
                            media_data.len() as f64 / 1000.0 / 100.00,
                            MAX_UPLOAD_SIZE_MEGABYTES
                        )
                            .as_str());
                        }
                        Err(RequestError::Api(teloxide::ApiError::Unknown(e))) => {
                            if e.contains("PHOTO_INVALID_DIMENSIONS") {
                                goodbye!(concat!(
                                    "Error: the resulting image's dimensions are invalid. ",
                                    "It's probably too small or the aspect ratio is too extreme. Sorry!"
                                ));
                            }
                            result
                        }
                        _ => result,
                    }
                })?;
                Ok(())
            }
            Task::Ocr => {
                let photo = data.message.get_media_info();
                let photo = match photo {
                    Some(photo) => {
                        if !photo.is_image() {
                            goodbye!(
                                "Error: can't work with video nor animated nor video stickers."
                            );
                        }
                        if photo.file.size > MAX_DOWNLOAD_SIZE_MEGABYTES * 1000 * 1000 {
                            goodbye!(format!(
                                "Error: image is too large. The limit is {MAX_DOWNLOAD_SIZE_MEGABYTES}MB."
                            )
                            .as_str());
                        }
                        photo
                    }
                    None => goodbye!("Error: can't find an image. "),
                };

                let mut photo_data: Vec<u8> = Vec::new();
                bot.download_file_to_vec(photo.file, &mut photo_data)
                    .await?;

                // Perform extraction.
                let woot =
                    tokio::task::spawn_blocking(move || media_processing::ocr_image(&photo_data))
                        .await
                        .expect("Worker died!");

                let mut text = match woot {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("Failed when OCRing: {e}");
                        goodbye!("Error: failed to process the media.");
                    }
                };

                if text.is_empty() {
                    goodbye!("Sorry, could not find any text.");
                }

                text.push_str("\n\n(automatically generated caption)");

                goodbye!(encode_text(&text).as_ref());
            }
            Task::AmenBreak => {
                let media = data.message.get_media_info();
                let media = match media {
                    Some(media) => {
                        if !media.is_raster() {
                            goodbye!(
                                "Error: can't work with animated stickers nor voice messages."
                            );
                        }
                        if media.is_sound {
                            goodbye!("Error: can't work with audio messages.");
                        }
                        if media.file.size > MAX_DOWNLOAD_SIZE_MEGABYTES * 1000 * 1000 {
                            goodbye!(format!(
                                "Error: media is too large. The limit is {MAX_DOWNLOAD_SIZE_MEGABYTES}MB."
                            )
                            .as_str());
                        }
                        media
                    }
                    None => goodbye!("Error: can't find the video or photo."),
                };

                let _ = status_report.send("Downloading media...".to_string());

                let download =
                    unerror_download!(bot.download_file_to_temp_or_directly(media.file).await);
                let path = download.0;
                let file = download.1;

                let status_report_for_processing = status_report.clone();

                let result = tokio::task::spawn_blocking(move || {
                    media_processing::amen_break_media(
                        status_report_for_processing,
                        &path,
                        media.is_video,
                    )
                })
                .await
                .expect("Worker died!");

                drop(file);

                let video_data = match result {
                    Ok(m) => m,
                    Err(e) => {
                        log::error!("Error when amen breaking video: {e}");
                        goodbye!("Error: failed to amen break the video");
                    }
                };

                if video_data.is_empty() {
                    goodbye!(
                        "Error: failed to amen break the video; got empty file as a result. Sorry!"
                    );
                }

                if video_data.len() > MAX_UPLOAD_SIZE_MEGABYTES as usize * 1000 * 1000 {
                    goodbye!(format!(
                        "Error: the resulting media is too big ({:.3}MB, max is {}MB). Sorry!",
                        video_data.len() as f64 / 1000.0 / 100.00,
                        MAX_UPLOAD_SIZE_MEGABYTES
                    )
                    .as_str());
                }

                let _ = status_report.send("Uploading result...".to_string());

                teloxide_retry!({
                    let send = video_data.clone();

                    bot.send_video(data.message.chat.id, InputFile::memory(send))
                        .reply_to(data.message.id)
                        .await
                })?;
                Ok(())
            }
            Task::Transcribe {
                temperature,
                translate_to_english,
            } => {
                let media = data.message.get_media_info();

                let media = match media {
                    Some(media) => {
                        if !(media.is_video || media.is_sound || media.is_voice_or_video_note) {
                            goodbye!("Error: input media doesn't have sound.");
                        }
                        if media.file.size > MAX_DOWNLOAD_SIZE_MEGABYTES * 1000 * 1000 {
                            goodbye!(format!(
                                "Error: media is too large. The limit is {MAX_DOWNLOAD_SIZE_MEGABYTES}MB."
                            )
                            .as_str());
                        }
                        media
                    }
                    None => goodbye!(concat!(
                        "Error: can't find a media with audio. ",
                        "This command needs to be used as either a reply or caption to one."
                    )),
                };

                let _ = status_report.send("Downloading media...".to_string());

                let download =
                    unerror_download!(bot.download_file_to_temp_or_directly(media.file).await);
                let path = download.0;
                let file = download.1;

                let _ = status_report.send("Extracting audio...".to_string());

                let wav = match tokio::task::spawn_blocking(move || {
                    whisper::convert_to_suitable_wav(&path)
                })
                .await
                .expect("Worker died!")
                {
                    Ok(wav) => wav,
                    Err(e) => {
                        log::error!("Error when converting to suitable wav: {e}");
                        goodbye!("Error: failed to extract audio. Does this media have any?");
                    }
                };

                drop(file);

                if wav.is_empty() {
                    goodbye!("Error: the input media has no audio.");
                }

                let _ = status_report.send("Transcribing...".to_string());

                let mut text = match whisper::submit_and_infer(
                    wav.into(),
                    *temperature,
                    *translate_to_english,
                )
                .await
                {
                    Ok(text) => text,
                    Err(e) => {
                        log::error!("Whisper infer failed: {e}");
                        goodbye!("Error: failed transcribing media.");
                    }
                };

                if text.is_empty() {
                    goodbye!("Sorry, could not transcribe anything.");
                }

                text.push_str("\n\n(automatically generated transcription)");

                goodbye!(encode_text(&text).as_ref());
            }
        }
    }
}
