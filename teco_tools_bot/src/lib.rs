mod entry;
mod handlers;
mod tasks;

pub use entry::*;

const USE_LOCAL_API: bool = cfg!(debug_assertions);
const OWNER_ID: teloxide::types::UserId = teloxide::types::UserId(1366743555);
const MAX_DOWNLOAD_SIZE_MEGABYTES: u32 = if USE_LOCAL_API { 150 } else { 20 };
const MAX_UPLOAD_SIZE_MEGABYTES: u32 = if USE_LOCAL_API { 2000 } else { 50 };
