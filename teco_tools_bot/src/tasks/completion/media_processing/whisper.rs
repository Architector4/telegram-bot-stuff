use std::{borrow::Cow, ffi::OsStr, io::Read, process::Command};

use reqwest::{Client, Error};
use tempfile::NamedTempFile;

pub async fn check_if_available() -> bool {
    let result = reqwest::get("http://127.0.0.1:9447/").await;
    if let Ok(response) = result {
        if response.status() == 200 {
            return true;
        }
    }

    log::error!("Whisper API broke.");
    false
}

pub async fn submit_and_infer(
    input_wav: Cow<'static, [u8]>,
    temperature: f32,
    transcribe_to_english: bool,
) -> Result<String, Error> {
    let temperature = temperature.max(0.0).min(1.0);
    let mut form_data = reqwest::multipart::Form::new()
        .part("file", reqwest::multipart::Part::bytes(input_wav))
        .text("response_format", "text");

    if temperature != 0.0 {
        form_data = form_data.text("temperature", temperature.to_string());
    }

    if transcribe_to_english {
        form_data = form_data.text("translate", "true");
    }

    let client = Client::new();
    let response = client
        .post("http://127.0.0.1:9447/inference")
        .multipart(form_data)
        .send()
        .await?
        .error_for_status()?;
    response.text().await
}

pub fn convert_to_suitable_wav(input: &std::path::Path) -> Result<Vec<u8>, std::io::Error> {
    // This needs to go through a temp file to let ffmpeg seek back and write
    // the correct "length" value into the header.
    let temp = NamedTempFile::new()?;
    let convert_result = Command::new("ffmpeg")
        .args([
            OsStr::new("-y"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-i"),
            input.as_ref(),
            OsStr::new("-ar"),
            OsStr::new("16000"),
            OsStr::new("-ac"),
            OsStr::new("1"),
            OsStr::new("-c:a"),
            OsStr::new("pcm_s16le"),
            OsStr::new("-f"),
            OsStr::new("wav"),
            temp.path().as_ref(),
        ])
        .spawn()?
        .wait()?;

    if !convert_result.success() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }

    let mut temp = temp.reopen()?;
    let mut output = Vec::with_capacity(temp.metadata()?.len() as usize);
    temp.read_to_end(&mut output)?;
    Ok(output)
}
