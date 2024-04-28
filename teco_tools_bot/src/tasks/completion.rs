use arch_bot_commons::useful_methods::*;
use magick_rust::{MagickError, MagickWand};
use teloxide::{
    payloads::{SendMessageSetters, SendPhotoSetters, SendStickerSetters},
    requests::Requester,
    types::InputFile,
    Bot, RequestError,
};
use tokio::time::sleep;

use super::{taskman::database::TaskDatabaseInfo, ImageFormat, ResizeType, Task};

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
            //Task::ToSticker => {
            //    use std::io::Cursor;
            //    let photo = data
            //        .message
            //        .get_photo_or_raster_sticker_here_or_reply_file_meta();

            //    let photo = match photo {
            //        Ok(Some(photo)) => photo,
            //        Ok(None) => goodbye!("Error: can't find an image"),
            //        Err(()) => goodbye!("Error: can't work with animated nor video stickers"),
            //    };

            //    let mut img_data: Vec<u8> = Vec::new();

            //    bot.download_file_to_vec(photo.2, &mut img_data).await?;

            //    let Some(image) = image::io::Reader::new(Cursor::new(&img_data))
            //        .with_guessed_format()
            //        .ok()
            //        .and_then(|x| x.decode().ok())
            //    else {
            //        goodbye!("Error: failed to parse the image");
            //    };

            //    let image = image.resize(512, 512, image::imageops::FilterType::CatmullRom);

            //    img_data.clear();

            //    let encoder =
            //        image::codecs::webp::WebPEncoder::new_lossless(Cursor::new(&mut img_data));
            //    image.write_with_encoder(encoder).unwrap();

            //    bot.send_sticker(data.message.chat.id, InputFile::memory(img_data))
            //        .reply_to_message_id(data.message.id.0)
            //        .await?;

            //    Ok(())
            //}
            Task::Resize {
                new_dimensions,
                percentage: _,
                mut format,
                resize_type,
            } => {
                let photo = data
                    .message
                    .get_photo_or_raster_sticker_here_or_reply_file_meta();

                let photo = match photo {
                    Ok(Some(photo)) => photo,
                    Ok(None) => goodbye!("Error: can't find an image"),
                    Err(()) => goodbye!("Error: can't work with animated nor video stickers"),
                };

                if format == ImageFormat::Preserve {
                    if photo.3 {
                        format = ImageFormat::Webp;
                    } else {
                        format = ImageFormat::Jpeg;
                    }
                }

                let mut img_data: Vec<u8> = Vec::new();

                bot.download_file_to_vec(photo.2, &mut img_data).await?;

                fn resize_image(
                    data: Vec<u8>,
                    width: usize,
                    height: usize,
                    resize_type: ResizeType,
                    format: ImageFormat,
                ) -> Result<(Vec<u8>, bool), MagickError> {
                    let wand = MagickWand::new();

                    wand.read_image_blob(data)?;

                    // The second and third arguments are "delta_x" and "rigidity"
                    // This library doesn't document them, but another bindings
                    // wrapper does: https://docs.wand-py.org/en/latest/wand/image.html
                    //
                    // According to it:
                    // > delta_x (numbers.Real) – maximum seam transversal step. 0 means straight seams. default is 0
                    // (but delta_x of 0 is very boring so we will pretend the default is 1)
                    // > rigidity (numbers.Real) – introduce a bias for non-straight seams. default is 0

                    // Also delta_x less than 0 segfaults. Other code prevents that from getting
                    // here, but might as well lol
                    // And both values in extremely high amounts segfault too, it seems lol

                    match resize_type {
                        ResizeType::SeamCarve { delta_x, rigidity } => {
                            wand.liquid_rescale_image(
                                width,
                                height,
                                delta_x.abs().min(4.0),
                                rigidity.max(-4.0).min(4.0),
                            )?;
                        }
                        ResizeType::Stretch => {
                            wand.resize_image(
                                width,
                                height,
                                magick_rust::bindings::FilterType_LagrangeFilter,
                            );
                        }
                        ResizeType::Fit | ResizeType::ToSticker => {
                            wand.fit(width, height);
                        }
                    }

                    let should_be_webp = match format {
                        ImageFormat::Preserve => wand.get_image_alpha_channel(),
                        ImageFormat::Webp => true,
                        ImageFormat::Jpeg => false,
                    };

                    Ok((
                        wand.write_image_blob(if should_be_webp { "webp" } else { "jpeg" })?,
                        should_be_webp,
                    ))
                }

                let dimensions = (
                    new_dimensions.0.get() as usize,
                    new_dimensions.1.get() as usize,
                );
                let resize_type = *resize_type;

                let woot = tokio::task::spawn_blocking(move || {
                    resize_image(img_data, dimensions.0, dimensions.1, resize_type, format)
                })
                .await
                .expect("Worker died!");
                let Ok((img_data, is_webp)) = woot else {
                    let wat = woot.unwrap_err();
                    log::error!("{}", wat);

                    goodbye!("Error: failed to parse the image");
                };

                let mut error: Result<(), RequestError> = Ok(());
                // Retry up to 3 times. Don't want to lose the cake lol
                for _ in 0..3 {
                    let send = img_data.clone();
                    let result = if is_webp {
                        bot.send_sticker(data.message.chat.id, InputFile::memory(send))
                            .reply_to_message_id(data.message.id.0)
                            .await
                    } else {
                        bot.send_photo(data.message.chat.id, InputFile::memory(send))
                            .reply_to_message_id(data.message.id)
                            .await
                    };
                    if let Err(e) = result {
                        if let RequestError::RetryAfter(x) = e {
                            sleep(x).await;
                        }
                        error = Err(e);
                    } else {
                        error = Ok(());
                        break;
                    }
                }

                error
            }
        }
    }
}
