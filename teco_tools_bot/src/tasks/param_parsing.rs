pub struct ParamParser<'a>(&'a str);

impl<'a> ParamParser<'a> {
    pub fn new(input: &str) -> ParamParser {
        ParamParser(input)
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

impl<'a> Iterator for ParamParser<'a> {
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

// Old impl of the iterator above lol
//impl ParamParser<'_> {
//    #[allow(clippy::bool_to_int_with_if)] // it's clearer lol
//    fn _amogus(&mut self) -> Option<<Self as Iterator>::Item> {
//        // If someone provides a string of "  , ,,  ,  , " or something,
//        // handle that gracefully lmao
//        loop {
//            self.0 = self.0.trim_start();
//
//            if self.0.starts_with(',') {
//                self.0 = &self.0[1..];
//            } else {
//                break;
//            }
//        }
//
//        if self.0.is_empty() {
//            return None;
//        }
//
//        let param_separator = self.0.find(|x| x == '\n' || x == ',');
//        let param_separator_offset = if param_separator.is_some() { 1 } else { 0 };
//
//        let mut keyval_separator = self.0.find(':');
//        if keyval_separator == Some(0) {
//            // If the first character is the param separator,
//            // we pretend it's part of the key lol
//            keyval_separator = None;
//        }
//
//        // 5 possible cases:
//        // 1. param_separator = None, keyval_separator = None,
//        // 2. param_separator = N, keyval_separator = None,
//        // 3. param_separator = None, keyval_separator = S,
//        // 4. param_separator = N, keyval_separator = S, N > S
//        // 5. param_separator = N, keyval_separator = S, N < S
//
//        // If there's no param separator, or it comes after the param_separator...
//        if keyval_separator.is_none()
//            || param_separator.map_or(false, |n| keyval_separator.unwrap() > n)
//        {
//            // ...then input is a wholesale string without key/val
//
//            // At which point the next argument (or lack thereof) begins
//            let separation = param_separator.unwrap_or(self.0.len());
//
//            let output = Some(Err(&self.0[..separation]));
//
//            // Consume up to then...
//            self.0 = &self.0[separation + param_separator_offset..];
//            return output;
//        };
//
//        // If the above check didn't return, then we MUST have a param separator.
//        // Cases 1 and 2 eliminated.
//        let keyval_separator = keyval_separator.unwrap();
//
//        // If the above check didn't return,
//        // then param separator MUST come before the param_separator.
//        // Case 5 is eliminated.
//
//        // Cases 3 and 4 are left. Either way...
//        assert!(param_separator.is_none() || param_separator.unwrap() > keyval_separator);
//
//        // ...input is a "key:val", maybe with a param_separator after it.
//
//        // At which point the next argument(or lack thereof) begins
//        let separation = param_separator.unwrap_or(self.0.len());
//
//        let key = &self.0[..keyval_separator];
//        let value = self.0[keyval_separator + 1..separation].trim();
//        let output = Some(Ok((key, value)));
//
//        // Consume input up to then
//        self.0 = &self.0[separation + param_separator_offset..];
//
//        output
//    }
//}
//
//#[cfg(test)]
//mod tests {
//    use super::*;
//    #[test]
//    fn test() {
//        let params = "aawagga";
//        let mut iterator = ParamParser::new(params);
//        assert_eq!(iterator.next(), Some(Err(params)));
//        assert_eq!(iterator.next(), None);
//
//        let params = "hi: hello";
//        let mut iterator = ParamParser::new(params);
//        assert_eq!(iterator.next(), Some(Ok(("hi", "hello"))));
//
//        let params = "hi: hello\nawa: aawagga";
//        let mut iterator = ParamParser::new(params);
//        assert_eq!(iterator.next(), Some(Ok(("hi", "hello"))));
//        assert_eq!(iterator.next(), Some(Ok(("awa", "aawagga"))));
//        assert_eq!(iterator.next(), None);
//
//        let params = "a: 1\n2\nc: 3\nd: 4\n5\n6\ng: 7";
//        let mut iterator = ParamParser::new(params);
//        assert_eq!(iterator.next(), Some(Ok(("a", "1"))));
//        assert_eq!(iterator.next(), Some(Err("2")));
//        assert_eq!(iterator.next(), Some(Ok(("c", "3"))));
//        assert_eq!(iterator.next(), Some(Ok(("d", "4"))));
//        assert_eq!(iterator.next(), Some(Err("5")));
//        assert_eq!(iterator.next(), Some(Err("6")));
//        assert_eq!(iterator.next(), Some(Ok(("g", "7"))));
//        assert_eq!(iterator.next(), None);
//
//        let params = ": what";
//        let mut iterator = ParamParser::new(params);
//        assert_eq!(iterator.next(), Some(Err(params)));
//        assert_eq!(iterator.next(), None);
//
//        let params = "mixed, yeah: hello\nwhat: amogus, yeah!\nfoo, bar\nbaz, boo: AAA, BBB";
//        let mut iterator = ParamParser::new(params);
//        assert_eq!(iterator.next(), Some(Err("mixed")));
//        assert_eq!(iterator.next(), Some(Ok(("yeah", "hello"))));
//        assert_eq!(iterator.next(), Some(Ok(("what", "amogus"))));
//        assert_eq!(iterator.next(), Some(Err("yeah!")));
//        assert_eq!(iterator.next(), Some(Err("foo")));
//        assert_eq!(iterator.next(), Some(Err("bar")));
//        assert_eq!(iterator.next(), Some(Err("baz")));
//        assert_eq!(iterator.next(), Some(Ok(("boo", "AAA"))));
//        assert_eq!(iterator.next(), Some(Err("BBB")));
//        assert_eq!(iterator.next(), None);
//
//        let params = ", ,,, , ,,  , ,, ,,,, , ,, ,,,, ,  ,,, , ,hi: hello";
//        let param = ParamParser::new(params).next();
//        assert_eq!(param, Some(Ok(("hi", "hello"))));
//    }
//}
