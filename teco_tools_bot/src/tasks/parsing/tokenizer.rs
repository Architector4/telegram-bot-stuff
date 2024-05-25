pub struct Tokenizer<'a>(&'a str);

impl<'a> Tokenizer<'a> {
    pub fn new(input: &str) -> Tokenizer {
        Tokenizer(input)
    }

    fn chomp_until_whitespace(&mut self) -> &'a str {
        self.0 = self.0.trim_start();

        let whitespace = self.0.find(char::is_whitespace);

        if let Some(s) = whitespace {
            let result = &self.0[0..s];
            self.0 = &self.0[s..];
            result
        } else {
            let result = self.0;
            // somewhat more logically correct but who cares
            //self.0 = &self.0[self.0.len()..];
            self.0 = "";
            result
        }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Result<(&'a str, &'a str), &'a str>;
    fn next(&mut self) -> Option<Self::Item> {
        let this_param = self.chomp_until_whitespace();

        if this_param.is_empty() {
            return None;
        }

        let keyval_separator = this_param.find(':');

        if let Some(kvs) = keyval_separator {
            let (a, b) = this_param.split_at(kvs);
            let mut b = &b[1..];

            if b.is_empty() {
                // Someone wrote "param: aawagga" and we got
                // the empty slice right after : but before the space
                // Eat one more word!
                b = self.chomp_until_whitespace();
            }

            Some(Ok((a, b)))
        } else {
            Some(Err(this_param))
        }
    }
}
