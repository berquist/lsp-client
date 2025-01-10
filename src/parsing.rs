//MIT License

//Copyright (c) 2017 Colin Rothfels

//Permission is hereby granted, free of charge, to any person obtaining a copy
//of this software and associated documentation files (the "Software"), to deal
//in the Software without restriction, including without limitation the rights
//to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//copies of the Software, and to permit persons to whom the Software is
//furnished to do so, subject to the following conditions:

//The above copyright notice and this permission notice shall be included in all
//copies or substantial portions of the Software.

//THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//SOFTWARE.

//! Handles parsing of Language Server Protocol messages from a stream.

use std::io::{self, BufRead};

use serde_json::value::Value;

macro_rules! print_err {
    ($($arg:tt)*) => (
        {
            use std::io::prelude::*;
            if let Err(e) = write!(&mut ::std::io::stderr(), "{}\n", format_args!($($arg)*)) {
                panic!("Failed to write to stderr.\
                    \nOriginal error output: {}\
                    \nSecondary error writing to stderr: {}", format!($($arg)*), e);
            }
        }
    )
}

#[allow(dead_code)]
#[derive(Debug)]
/// An Error type encapsulating the various failure possibilites of the parsing process.
pub enum ParseError {
    Io(io::Error),
    ParseInt(std::num::ParseIntError),
    Utf8(std::string::FromUtf8Error),
    Encoding(String),
    Json(serde_json::Error),
    Unknown(String),
    Empty,
}

impl From<io::Error> for ParseError {
    fn from(err: io::Error) -> ParseError {
        ParseError::Io(err)
    }
}

impl From<std::string::FromUtf8Error> for ParseError {
    fn from(err: std::string::FromUtf8Error) -> ParseError {
        ParseError::Utf8(err)
    }
}

impl From<serde_json::Error> for ParseError {
    fn from(err: serde_json::Error) -> ParseError {
        ParseError::Json(err)
    }
}

impl From<std::num::ParseIntError> for ParseError {
    fn from(err: std::num::ParseIntError) -> ParseError {
        ParseError::ParseInt(err)
    }
}

impl From<String> for ParseError {
    fn from(s: String) -> ParseError {
        ParseError::Unknown(s)
    }
}

#[derive(Debug, PartialEq)]
/// A message header, as described in the Language Server Protocol specification.
enum LspHeader {
    ContentType,
    ContentLength(usize),
}

/// Given a reference to a reader, attempts to read a Language Server Protocol message,
/// blocking until a message is received.
pub fn read_message<B: BufRead>(reader: &mut B) -> Result<Value, ParseError> {
    let mut buffer = String::new();
    let mut content_length: Option<usize> = None;

    // read in headers.
    loop {
        buffer.clear();
        let num_bytes = reader.read_line(&mut buffer)?;
        if num_bytes == 0 {
            return Err(ParseError::Empty);
        }
        // assert!(buffer.ends_with("\r\n"));
        match &buffer {
            s if s.trim().is_empty() => break, // empty line is end of headers
            // s if s == "\r\n" => break, // empty line is end of headers
            s => {
                match parse_header(s)? {
                    LspHeader::ContentLength(len) => content_length = Some(len),
                    LspHeader::ContentType => (), // utf-8 only currently allowed value
                };
            }
        };
    }

    let content_length =
        content_length.ok_or(format!("missing content-length header: {}", buffer))?;
    // message body isn't newline terminated, so we read content_length bytes
    let mut body_buffer = vec![0; content_length];
    reader.read_exact(&mut body_buffer)?;
    let body = String::from_utf8(body_buffer)?;
    Ok(serde_json::from_str(&body)?)
}

const HEADER_CONTENT_LENGTH: &str = "content-length";
const HEADER_CONTENT_TYPE: &str = "content-type";

/// Given a header string, attempts to extract and validate the name and value parts.
fn parse_header(s: &str) -> Result<LspHeader, ParseError> {
    let split: Vec<String> = s.split(": ").map(|s| s.trim().to_lowercase()).collect();
    if split.len() != 2 {
        return Err(ParseError::Unknown(format!("malformed header: {}", s)));
    }
    match split[0].as_ref() {
        HEADER_CONTENT_TYPE => {
            let encoding = split[1].to_lowercase();
            let is_valid_encoding = ["utf-8", "utf8"]
                .iter()
                .any(|valid_encoding| *valid_encoding == encoding);
            if is_valid_encoding {
                Ok(LspHeader::ContentType)
            } else {
                Err(ParseError::Encoding(format!(
                    "Invalid encoding: {}",
                    split[1]
                )))
            }
        }
        HEADER_CONTENT_LENGTH => Ok(LspHeader::ContentLength(split[1].parse()?)),
        _ => Err(ParseError::Unknown(format!("Unknown header: {}", s))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn test_parse_header_content_length() {
        let header = "Content-Length: 132";
        assert_eq!(
            parse_header(header).ok(),
            Some(LspHeader::ContentLength(132))
        );
    }

    #[test]
    fn test_parse_header_content_type() {
        let header = "Content-Type: utf-8";
        let parsed = parse_header(header);
        assert_eq!(parsed.ok(), Some(LspHeader::ContentType));

        // For backwards compatibility; see
        // https://microsoft.github.io/language-server-protocol/specifications/specification-3-16/#contentPart
        let header = "Content-Type: utf8";
        let parsed = parse_header(header);
        assert_eq!(parsed.ok(), Some(LspHeader::ContentType));
    }

    #[test]
    fn test_parse_header_invalid_content_type() {
        let header = "Content-Type: ascii";
        let parsed = parse_header(header);
        assert!(parsed.is_err());
    }

    #[test]
    fn test_parse_header_malformed() {
        let test_cases = [
            ("", "malformed header: "),
            ("Content-Length:132", "malformed header: Content-Length:132"),
        ];
        for (header, err_msg) in test_cases {
            let parsed_header = parse_header(header);
            assert_eq!(parsed_header.as_ref().ok(), None);
            match parsed_header.as_ref().err().unwrap() {
                ParseError::Unknown(s) => {
                    assert_eq!(*s, err_msg.to_string())
                }
                default => panic!("incorrect ParseError variant: {:#?}", default),
            }
        }
    }

    #[test]
    fn test_parse_header_unknown() {
        let header = "Hello: world";
        let parsed_header = parse_header(header);
        assert_eq!(parsed_header.as_ref().ok(), None);
        match parsed_header.as_ref().err().unwrap() {
            ParseError::Unknown(s) => assert_eq!(*s, "Unknown header: Hello: world".to_string()),
            default => panic!("incorrect ParseError variant: {:#?}", default),
        }
    }

    #[test]
    fn test_parse_error() {
        let header = "Content-Length: 132 hi";
        let parsed_header = parse_header(header);
        assert_eq!(parsed_header.as_ref().ok(), None);
        match parsed_header.as_ref().err().unwrap() {
            ParseError::ParseInt(s) => println!("{:#?}", s),
            default => panic!("incorrect ParseError variant: {:#?}", default),
        }
    }

    #[test]
    fn test_read_message() {
        let inps = vec![
            "Content-Length: 18\n\r\n\r{\"name\": \"value\"}",
            "Content-length: 18\n\r\n\r{\"name\": \"value\"}",
            "Content-Length: 18\n\rContent-Type: utf-8\n\r\n\r{\"name\": \"value\"}",
            "Content-Length: 18\n\rContent-Type: utf-8\n\r\n\r{\"name\": \"value\"}\n",
        ];
        for inp in inps {
            let mut reader = BufReader::new(inp.as_bytes());
            let result = match read_message(&mut reader) {
                Ok(r) => r,
                Err(e) => panic!("unexpected error: {:#?}", e),
            };
            let exp = json!({"name": "value"});
            assert_eq!(result, exp);
        }
    }

    #[test]
    fn test_read_message_invalid_content_type() {
        let test_cases = [
            (
                "Content-Length: 18\n\rContent-Type: ascii\n\r\n\r{\"name\": \"value\"}",
                "Invalid encoding: ascii",
            ),
            (
                "Content-Length: 18\n\rContent-Type: utf-9\n\r\n\r{\"name\": \"hello\"}",
                "Invalid encoding: utf-9",
            ),
        ];
        for (inp, err_msg) in test_cases {
            let mut reader = BufReader::new(inp.as_bytes());
            let result = match read_message(&mut reader) {
                Ok(r) => panic!("unexpected success: {:#?}", r),
                Err(e) => match e {
                    ParseError::Encoding(s) => {
                        assert_eq!(s, err_msg.to_string())
                    }
                    default => panic!("incorrect ParseError variant: {:#?}", default),
                },
            };
            assert_eq!(result, ());
        }
    }

    #[test]
    fn test_read_message_missing_content_length() {
        let test_cases = [
            // Without the \n\r\n\r this leads to a failed header parse.
            (
                "\n\r\n\r{\"name\": \"value\"}",
                "missing content-length header: \n",
            ),
            (
                "Content-Type: utf-8\n\r\n\r{\"name\": \"value\"}",
                "missing content-length header: \r\n",
            ),
        ];
        for (inp, err_msg) in test_cases {
            let mut reader = BufReader::new(inp.as_bytes());
            let result = match read_message(&mut reader) {
                Ok(r) => panic!("unexpected success: {:#?}", r),
                Err(e) => match e {
                    ParseError::Unknown(s) => {
                        assert_eq!(s, err_msg.to_string())
                    }
                    default => panic!("incorrect ParseError variant: {:#?}", default),
                },
            };
            assert_eq!(result, ());
        }
    }
}
