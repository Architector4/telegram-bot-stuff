mod split_msg;
pub use split_msg::*;

use futures::{Future, TryStreamExt};
use teloxide::{
    net::Download,
    requests::Requester,
    types::{ChatId, FileMeta, Message, PhotoSize},
    Bot, RequestError,
};

pub struct MessageMediaInfo<'a> {
    pub width: u32,
    pub height: u32,
    pub is_sticker: bool,
    pub is_gif: bool,
    pub is_video: bool,
    pub is_vector_sticker: bool,
    pub file: &'a FileMeta,
}

impl<'a> MessageMediaInfo<'a> {
    pub fn is_image(&self) -> bool {
        !self.is_video && !self.is_vector_sticker
    }
}

pub trait MessageStuff {
    fn text_full(&self) -> Option<&str>;
    #[allow(clippy::result_unit_err)] // i'm lazy lol
    /// On success, returns width, height and file metadata of the image,
    /// as well as bool that is `true` if it's a sticker.
    ///
    /// # Errors
    /// Returns Err(()) if there is a sticker but it's not raster.
    fn get_media_info(&self) -> Option<MessageMediaInfo<'_>>;
    fn find_biggest_photo(&self) -> Option<&PhotoSize>;
}

impl MessageStuff for Message {
    fn text_full(&self) -> Option<&str> {
        self.text().or_else(|| self.caption())
    }
    fn get_media_info(&self) -> Option<MessageMediaInfo<'_>> {
        if let Some(biggest) = self.find_biggest_photo() {
            return Some(MessageMediaInfo {
                width: biggest.width,
                height: biggest.height,
                is_sticker: false,
                is_gif: false,
                is_video: false,
                is_vector_sticker: false,
                file: &biggest.file,
            });
        }

        if let Some(sticker) = self.sticker() {
            return Some(MessageMediaInfo {
                width: sticker.width.into(),
                height: sticker.height.into(),
                is_sticker: true,
                is_gif: false,
                is_video: sticker.is_video(),
                is_vector_sticker: sticker.is_animated(),
                file: &sticker.file,
            });
        }

        if let Some(video) = self.video() {
            return Some(MessageMediaInfo {
                width: video.width,
                height: video.height,
                is_sticker: false,
                is_gif: false,
                is_video: true,
                is_vector_sticker: false,
                file: &video.file,
            });
        }

        if let Some(animation) = self.animation() {
            return Some(MessageMediaInfo {
                width: animation.width,
                height: animation.height,
                is_sticker: false,
                is_video: true,
                is_gif: true,
                is_vector_sticker: false,
                file: &animation.file,
            });
        }

        if let Some(reply_to) = self.reply_to_message() {
            return reply_to.get_media_info();
        }

        None
    }
    fn find_biggest_photo(&self) -> Option<&PhotoSize> {
        if let Some(photo_sizes) = self.photo() {
            photo_sizes.iter().max_by_key(|x| x.width + x.height)
        } else {
            None
        }
    }
}

pub trait BotStuff {
    fn download_file_to_vec(
        &self,
        file: &FileMeta,
        to: &mut Vec<u8>,
    ) -> impl Future<Output = Result<(), RequestError>> + Send;

    fn typing(&self, to_where: ChatId) -> impl Future<Output = Result<(), RequestError>> + Send;
}

impl BotStuff for Bot {
    async fn download_file_to_vec(
        &self,
        file: &FileMeta,
        to: &mut Vec<u8>,
    ) -> Result<(), RequestError> {
        let file = self.get_file(&file.id).await?;
        to.reserve_exact(file.size as usize);
        let mut stream = self.download_file_stream(&file.path);

        while let Some(bytes) = stream.try_next().await? {
            for byte in bytes {
                to.push(byte);
            }
        }

        Ok(())
    }
    async fn typing(&self, to_where: ChatId) -> Result<(), RequestError> {
        self.send_chat_action(to_where, teloxide::types::ChatAction::Typing)
            .await?;
        Ok(())
    }
}
