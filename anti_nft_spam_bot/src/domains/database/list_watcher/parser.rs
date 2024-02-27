use std::{
    fs::File,
    io::{BufRead, BufReader, Error, ErrorKind},
};

use url::Url;

use crate::domains::types::Domain;

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
        let Some(domain_str) = split.next() else {
            // Empty line. Meh.
            return Some(Ok(None));
        };

        if domain_str.as_bytes()[0] == b'#' {
            // Comment line. Meh.
            return Some(Ok(None));
        }

        let example_url_str = split.next();

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

        // We now have a domain name and an example URL, as strings...

        let Some(domain) = Domain::from_str(domain_str) else {
            return Some(Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Could not parse domain in line {}:\n{}",
                    self.line_counter, self.buffer
                ),
            )));
        };

        let example_url = example_url_str.map(Url::parse);

        if let Some(Err(_)) = example_url {
            return Some(Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Could not parse example URL in line {}:\n{}",
                    self.line_counter, self.buffer
                ),
            )));
        };

        let example_url = example_url.map(Result::unwrap);

        Some(Ok(Some(Line {
            domain,
            example_url,
        })))
    }
}

#[derive(Debug, Clone)]
pub struct Line {
    pub domain: Domain,
    pub example_url: Option<Url>,
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
