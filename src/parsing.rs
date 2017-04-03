//! Handles parsing of Language Server Protocol messages from a stream.

use std;
use std::io::{self, BufRead, BufReader};

use serde_json;
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

#[derive(Debug)]
/// An Error type encapsulating the various failure possibilites of the parsing process.
enum ParseError {
    Io(io::Error),
    ParseInt(std::num::ParseIntError),
    Utf8(std::string::FromUtf8Error),
    Json(serde_json::Error),
    Unknown(String),
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
fn read_message<B: BufRead>(reader: &mut B) -> Result<Value, ParseError> {
    let mut buffer = String::new();
    let mut content_length: Option<usize> = None;

    // read in headers. 
    loop {
            reader.read_line(&mut buffer)?;
            match &buffer {
                s if s.trim().len() == 0 => { break }, // empty line is end of headers
                s => {
                    match parse_header(s)? {
                        LspHeader::ContentLength(len) => content_length = Some(len),
                        LspHeader::ContentType => (), // utf-8 only currently allowed value
                    };
                }
            };
            buffer.clear();
        }
    
    let content_length = content_length.ok_or("missing Content-Length header".to_owned())?;
    // message body isn't newline terminated, so we read content_length bytes
    let mut body_buffer = vec![0; content_length];
    reader.read_exact(&mut body_buffer)?;
    let body = String::from_utf8(body_buffer)?;
    Ok(serde_json::from_str::<Value>(&body)?)
}

const HEADER_CONTENT_LENGTH: &'static str = "content-length";
const HEADER_CONTENT_TYPE: &'static str = "content-type";

/// Given a header string, attempts to extract and validate the name and value parts.
fn parse_header(s: &str) -> Result<LspHeader, ParseError> {
    let split: Vec<String> = s.split(": ").map(|s| s.trim().to_lowercase()).collect();
    if split.len() != 2 { return Err(ParseError::Unknown(format!("malformed header: {}", s))) }
    match split[0].as_ref() {
        HEADER_CONTENT_TYPE => Ok(LspHeader::ContentType),
        HEADER_CONTENT_LENGTH => Ok(LspHeader::ContentLength(usize::from_str_radix(&split[1], 10)?)),
        _ => Err(ParseError::Unknown(format!("Unknown header: {}", s))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_header() {
        let header = "Content-Length: 132";
        assert_eq!(parse_header(header).ok(), Some((LspHeader::ContentLength(132))));
    }

    #[test]
    fn test_parse_message() {
        let inps = vec!("Content-Length: 18\n\r\n\r{\"name\": \"value\"}", 
                        "Content-length: 18\n\r\n\r{\"name\": \"value\"}", 
                        "Content-Length: 18\n\rContent-Type: utf-8\n\r\n\r{\"name\": \"value\"}");
        for inp in inps {
            let mut reader = BufReader::new(inp.as_bytes());
            let result = match read_message(&mut reader) {
                Ok(r) => r,
                Err(e) => panic!("error: {:?}", e),
            };
            let exp = json!({"name": "value"});
            assert_eq!(result, exp);
        }
    }
}