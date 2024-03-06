use std::{
    fs::File,
    io::{BufRead, BufReader, Error, ErrorKind},
};

use url::Url;

use crate::{domains::types::Domain, parse_url_like_telegram};

#[derive(Debug)]
pub struct Parser {
    reader: Option<BufReader<File>>,
    buffer: String,
    line_counter: usize,
}

impl Parser {
    pub fn new(reader: BufReader<File>) -> Self {
        Self {
            reader: Some(reader),
            buffer: String::with_capacity(256),
            line_counter: 0,
        }
    }

    /// Parse the next line.
    ///
    /// If [`None`] is returned, this means the file has ended.
    ///
    /// If `Some(Ok(None))` returned, this means that the line is empty.
    pub fn next_line(&mut self) -> Option<Result<Option<Line>, Error>> {
        let Some(reader) = &mut self.reader else {
            // Reader end.
            return None;
        };

        self.buffer.clear();
        let bytes_read = match reader.read_line(&mut self.buffer) {
            Ok(b) => b,
            Err(e) => return Some(Err(e)),
        };

        if bytes_read == 0 {
            // File end.
            self.reader = None;
            return None;
        }

        // Line read successfully. Bump the counter...
        self.line_counter += 1;

        // Trim extraneous newlines and whatnot...
        let line = self.buffer.trim_end();

        // Trim a comment...
        let comment_start = line.find(" #").unwrap_or(line.len());
        let line = &line[0..comment_start];

        let mut split = line.split_whitespace();
        let Some(line_type) = split.next() else {
            // Empty line. Meh.
            return Some(Ok(None));
        };

        if line_type.as_bytes()[0] == b'#' {
            // Comment line. Meh.
            return Some(Ok(None));
        }

        let Some(url_str) = split.next() else {
            // Only a type but no data? No good, line is weird lol
            return Some(Err(Error::new(
                ErrorKind::InvalidData,
                format!("No URL in line {}:\n{}", self.line_counter, self.buffer),
            )));
        };

        if split.next().is_some() {
            // Third parameter? Not good, line is weird lol
            return Some(Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Extraneous parameter in line {}:\n{}",
                    self.line_counter, self.buffer
                ),
            )));
        }

        let url = match parse_url_like_telegram(url_str) {
            Ok(url) => url,
            Err(e) => {
                return Some(Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "Could not parse URL in line {}:\n{}\n{}",
                        self.line_counter, self.buffer, e
                    ),
                )));
            }
        };

        // We now have a type of line, and a URL.

        let line = match Line::try_from((line_type, url)) {
            Ok(line) => line,
            Err(e) => {
                // Failed to parse line type...
                return Some(Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "Failed to parse type in line {}:\n{}\n{}",
                        self.line_counter, self.buffer, e
                    ),
                )));
            }
        };

        Some(Ok(Some(line)))
    }
}

#[derive(Debug, Clone)]
pub enum Line {
    // It's fine for now...
    #[allow(dead_code)]
    Url(Url),
    Domain {
        domain: Domain,
        example_url: Url,
    },
}

impl TryFrom<(&str, Url)> for Line {
    type Error = &'static str;
    fn try_from((line_type, url): (&str, Url)) -> Result<Self, Self::Error> {
        match line_type {
            "domain" => match Domain::from_url(&url) {
                Some(domain) => Ok(Self::Domain {
                    domain,
                    example_url: url,
                }),
                None => Err("No domain in URL"),
            },
            // TODO: enable this when it's done...
            //"url" => Ok(Self::Url(url)),
            _ => Err("Unknown line type"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufReader};

    #[test]
    fn validate_data() {
        use super::super::LIST_FILE;

        let file = File::open(LIST_FILE).unwrap();
        let reader = BufReader::new(file);

        let mut parser = super::Parser::new(reader);

        while let Some(line) = parser.next_line() {
            if let Some(line) = line.unwrap() {
                println!("{:?}", line);
            }
        }
    }
}
