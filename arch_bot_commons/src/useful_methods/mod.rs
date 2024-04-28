mod split_msg;
pub use split_msg::*;

use futures::{Future, TryStreamExt};
use teloxide::{
    net::Download,
    requests::Requester,
    types::{ChatId, FileMeta, Message, PhotoSize},
    Bot, RequestError,
};

pub trait MessageStuff {
    fn text_full(&self) -> Option<&str>;
    #[allow(clippy::result_unit_err)] // i'm lazy lol
    /// On success, returns width, height and file metadata of the image,
    /// as well as bool that is `true` if it's a sticker.
    ///
    /// # Errors
    /// Returns Err(()) if there is a sticker but it's not raster.
    fn get_photo_or_raster_sticker_here_or_reply_file_meta(
        &self,
    ) -> Result<Option<(u32, u32, &FileMeta, bool)>, ()>;
    fn find_biggest_photo(&self) -> Option<&PhotoSize>;
}

impl MessageStuff for Message {
    fn text_full(&self) -> Option<&str> {
        self.text().or_else(|| self.caption())
    }
    fn get_photo_or_raster_sticker_here_or_reply_file_meta(
        &self,
    ) -> Result<Option<(u32, u32, &FileMeta, bool)>, ()> {
        if let Some(biggest) = self.find_biggest_photo() {
            return Ok(Some((biggest.width, biggest.height, &biggest.file, false)));
        }
        if let Some(sticker) = self.sticker() {
            if !sticker.is_raster() {
                return Err(());
            }

            return Ok(Some((
                sticker.width.into(),
                sticker.height.into(),
                &sticker.file,
                true,
            )));
        }

        if let Some(reply_to) = self.reply_to_message() {
            return reply_to.get_photo_or_raster_sticker_here_or_reply_file_meta();
        }

        Ok(None)
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
