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

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin};
use std::sync::{Arc, Mutex};
use std::thread;

use jsonrpc_lite::JsonRpc as JsonRPC;
use serde_json::{self, value::Value};

use crate::parsing::{self, ParseError};

// this to get around some type system pain related to callbacks. See:
// https://doc.rust-lang.org/beta/book/trait-objects.html,
// http://stackoverflow.com/questions/41081240/idiomatic-callbacks-in-rust
trait Callable: Send {
    fn call(self: Box<Self>, result: Result<Value, Value>);
}

impl<F: Send + FnOnce(Result<Value, Value>)> Callable for F {
    fn call(self: Box<F>, result: Result<Value, Value>) {
        (*self)(result)
    }
}

type Callback = Box<dyn Callable>;

/// Represents (and mediates communcation with) a Language Server.
///
/// LanguageServer should only ever be instantiated or accessed through an instance of
/// LanguageServerRef, which mediates access to a single shared LanguageServer through a Mutex.
struct LanguageServer<W: Write> {
    peer: W,
    pending: HashMap<usize, Callback>,
    next_id: usize,
}

/// Generates a Language Server Protocol compliant message.
fn prepare_lsp_json(msg: &Value) -> Result<String, serde_json::error::Error> {
    let request = serde_json::to_string(&msg)?;
    Ok(format!(
        "Content-Length: {}\r\n\r\n{}",
        request.len(),
        request
    ))
}

#[allow(dead_code)]
impl<W: Write> LanguageServer<W> {
    fn write(&mut self, msg: &str) {
        self.peer
            .write_all(msg.as_bytes())
            .expect("error writing to stdin");
        self.peer.flush().expect("error flushing child stdin");
    }

    fn send_request(&mut self, method: &str, params: &Value, completion: Callback) {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params
        });

        self.pending.insert(self.next_id, completion);
        self.next_id += 1;
        self.send_rpc(&request);
    }

    fn send_notification(&mut self, method: &str, params: &Value) {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.send_rpc(&notification);
    }

    fn handle_response(&mut self, id: usize, result: Value) {
        let callback = self
            .pending
            .remove(&id)
            .unwrap_or_else(|| panic!("id {} missing from request table", id));
        callback.call(Ok(result));
    }

    fn handle_error(&mut self, id: usize, error: Value) {
        let callback = self
            .pending
            .remove(&id)
            .unwrap_or_else(|| panic!("id {} missing from request table", id));
        callback.call(Err(error));
    }

    fn send_rpc(&mut self, rpc: &Value) {
        let rpc = match prepare_lsp_json(rpc) {
            Ok(r) => r,
            Err(err) => panic!("error encoding rpc {:?}", err),
        };
        self.write(&rpc);
    }
}

/// Access control and convenience wrapper around a shared LanguageServer instance.
pub struct LanguageServerRef<W: Write>(Arc<Mutex<LanguageServer<W>>>);

//FIXME: this is hacky, and prevents good error propagation,
fn number_from_id(id: Option<&Value>) -> usize {
    let id = id.expect("response missing id field");
    let id = match id {
        Value::Number(n) => n.as_u64().expect("failed to take id as u64"),
        Value::String(s) => s.parse().expect("failed to convert string id to u64"),
        other => panic!("unexpected value for id field: {:?}", other),
    };

    id as usize
}

#[allow(dead_code)]
impl<W: Write> LanguageServerRef<W> {
    fn new(peer: W) -> Self {
        LanguageServerRef(Arc::new(Mutex::new(LanguageServer {
            peer,
            pending: HashMap::new(),
            next_id: 1,
        })))
    }

    /// Writes `msg` to the underlying process's stdin. Exposed for testing & debugging;
    /// you should not need to call this method directly.
    fn write(&self, msg: &str) {
        let mut inner = self.0.lock().unwrap();
        inner.write(msg);
    }

    //TODO: real logging (with slog?)
    fn handle_msg(&self, val: &Value) {
        // TODO avoid what looks like a round trip
        let parsed = val.to_string();
        let parsed = JsonRPC::parse(&parsed).expect("problem creating JsonRpc instance");
        match parsed {
            JsonRPC::Request(obj) => print_err!("client received unexpected request: {:?}", obj),
            JsonRPC::Notification(obj) => println!("recv notification: {:?}", obj),
            JsonRPC::Success(_) => {
                let mut inner = self.0.lock().unwrap();
                let id = number_from_id(val.get("id"));
                let _ = val.get("result").expect("response missing 'result' field");
                // TODO clone
                inner.handle_response(id, val.clone());
            }
            JsonRPC::Error(_) => {
                // TODO I'm not sure why this was this way before.
                // if val.get("id").expect("error missing id field").is_null() {
                //     let mut inner = self.0.lock().unwrap();
                //     // TODO clone
                //     inner.handle_error(number_from_id(val.get("id")), val.clone());
                // } else {
                //     print_err!("received error: {:?}", obj);
                // }
                let mut inner = self.0.lock().unwrap();
                inner.handle_error(number_from_id(val.get("id")), val.clone());
            }
        }
    }

    /// Sends a JSON-RPC request message with the provided method and parameters.
    /// `completion` should be a callback which will be executed with the server's response.
    pub fn send_request<CB>(&self, method: &str, params: &Value, completion: CB)
    where
        CB: 'static + Send + FnOnce(Result<Value, Value>),
    {
        let mut inner = self.0.lock().unwrap();
        inner.send_request(method, params, Box::new(completion));
    }

    /// Sends a JSON-RPC notification message with the provided method and parameters.
    pub fn send_notification(&self, method: &str, params: &Value) {
        let mut inner = self.0.lock().unwrap();
        inner.send_notification(method, params);
    }
}

impl<W: Write> Clone for LanguageServerRef<W> {
    fn clone(&self) -> Self {
        LanguageServerRef(self.0.clone())
    }
}

pub fn start_language_server(mut child: Child) -> (Child, LanguageServerRef<ChildStdin>) {
    let child_stdin = child.stdin.take().unwrap();
    let child_stdout = child.stdout.take().unwrap();
    let lang_server = LanguageServerRef::new(child_stdin);
    {
        let lang_server = lang_server.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(child_stdout);
            loop {
                match parsing::read_message(&mut reader) {
                    Ok(ref val) => lang_server.handle_msg(val),
                    Err(ParseError::Empty) => (),
                    Err(err) => eprintln!("parse error: {:?}", err),
                };
            }
        });
    }
    (child, lang_server)
}

#[cfg(test)]
mod tests {
    use std::{
        process::{Child, Command, Stdio},
        sync::mpsc,
    };

    use super::*;

    fn prepare_command() -> Child {
        Command::new("rust-analyzer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to start language server")
    }

    #[test]
    fn test_start_language_server() {
        let (mut child, lang_server) = start_language_server(prepare_command());

        let (tx, rx) = mpsc::channel();
        let init = json!({
            "process_id": "Null",
            "initialization_options": {},
            "capabilities": {},
        });
        lang_server.send_request("initialize", &init, move |result| {
            let _ = tx.send(result);
        });
        let initialize_result = rx.recv().expect("problem receiving from channel");
        println!("received response {initialize_result:#?}");
        assert!(initialize_result.is_ok());
        assert!(initialize_result.as_ref().err().is_none());
        let initialize_result = initialize_result.unwrap();
        let error = initialize_result.get("error");
        assert!(error.is_none());

        let initialized = json!({});
        lang_server.send_notification("initialized", &initialized);

        let (tx, rx) = mpsc::channel();
        let shutdown = json!(());
        lang_server.send_request("shutdown", &shutdown, move |result| {
            let _ = tx.send(result);
        });
        let shutdown_result = rx.recv().expect("problem receiving from channel");
        println!("received response {shutdown_result:#?}");
        assert!(shutdown_result.is_ok());
        assert!(shutdown_result.as_ref().err().is_none());
        let shutdown_result = shutdown_result.unwrap();
        let error = shutdown_result.get("error");
        assert!(error.is_none());

        let exit = json!({});
        lang_server.send_notification("exit", &exit);

        let _ = child.wait();
    }

    #[test]
    fn test_language_server_bad_arguments() {
        let (mut child, lang_server) = start_language_server(prepare_command());

        let (tx, rx) = mpsc::channel();
        let init = json!({
            "process_id": "Null",
            "initialization_options": {},
            "capabilities": {},
        });
        lang_server.send_request("initialize", &init, move |result| {
            let _ = tx.send(result);
        });
        let initialize_result = rx.recv().expect("problem receiving from channel");
        println!("received response {initialize_result:#?}");
        assert!(initialize_result.is_ok());
        assert!(initialize_result.as_ref().err().is_none());
        let initialize_result = initialize_result.unwrap();
        let error = initialize_result.get("error");
        assert!(error.is_none());

        let initialized = json!({});
        lang_server.send_notification("initialized", &initialized);

        let (tx, rx) = mpsc::channel();
        // should be null, not a map, even an empty one
        let shutdown = json!({});
        lang_server.send_request("shutdown", &shutdown, move |result| {
            let _ = tx.send(result);
        });
        let shutdown_result = rx.recv().expect("problem receiving from channel");
        println!("received response {shutdown_result:#?}");
        assert!(shutdown_result.is_err());
        assert!(shutdown_result.as_ref().ok().is_none());
        let shutdown_result = shutdown_result.err().unwrap();
        let error = shutdown_result.get("error");
        assert!(error.is_some());

        // we can still exist normally
        let exit = json!({});
        lang_server.send_notification("exit", &exit);

        let _ = child.wait();
    }
}
