//! Read snapshot v0 JSON from stdin and print compact selection JSON.

use agentmesh_multica_selector_shadow::{parse_input, select_compact_payload};
use std::io::{self, Read};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut raw = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut raw) {
        eprintln!("stdin_read_error:{err}");
        return ExitCode::from(2);
    }
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("json_parse_error:{err}");
            return ExitCode::from(2);
        }
    };
    match parse_input(&value) {
        Ok(input) => {
            let payload = select_compact_payload(&input);
            match serde_json::to_string(&payload) {
                Ok(text) => {
                    println!("{text}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("json_encode_error:{err}");
                    ExitCode::from(2)
                }
            }
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
    }
}
