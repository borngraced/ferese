use std::env;
use std::error::Error;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use ferese_ipc::{Request, Response, VERSION, read_frame, write_frame};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn Error>> {
    let (command, args) = parse_args(env::args().skip(1))?;
    let request = Request {
        version: VERSION,
        id: 1,
        kind: "command".to_owned(),
        command,
        args,
    };
    let mut stream = UnixStream::connect(socket_path()?)?;

    write_frame(&mut stream, &request)?;
    let response: Response = read_frame(&mut stream)?;
    if let Some(error) = response.error {
        return Err(format!("{}: {}", error.code, error.message).into());
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&response.result.unwrap_or(Value::Null))?
    );
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<(String, Value), String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or_else(usage)?;
    let positional = args.collect::<Vec<_>>();
    let payload = match command.as_str() {
        "focus" | "move" | "resize" => {
            exactly_one(&command, &positional, "direction")?;
            json!({ "direction": positional[0] })
        }
        "workspace" | "move-to-workspace" => {
            exactly_one(&command, &positional, "index")?;
            let index = positional[0]
                .parse::<u32>()
                .map_err(|_| format!("{} requires a positive workspace index", command))?;
            json!({ "index": index })
        }
        "toggle-floating" | "toggle-fullscreen" | "toggle-layout" | "cycle-column-width"
        | "center-column" | "consume" | "expel" | "close" | "get-focused-window"
        | "get-workspaces" | "get-outputs" => {
            if !positional.is_empty() {
                return Err(format!("{command} does not accept arguments"));
            }
            json!({})
        }
        _ => return Err(format!("unknown command {command:?}\n{}", usage())),
    };

    Ok((command, payload))
}

fn exactly_one(command: &str, args: &[String], name: &str) -> Result<(), String> {
    if args.len() == 1 {
        Ok(())
    } else {
        Err(format!("{command} requires exactly one {name}"))
    }
}

fn socket_path() -> Result<PathBuf, io::Error> {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|directory| directory.join("ferese/control.sock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))
}

fn usage() -> String {
    "usage: feresectl <focus|move|resize> <direction>\n       feresectl <workspace|move-to-workspace> <index>\n       feresectl <toggle-floating|toggle-fullscreen|toggle-layout>\n       feresectl <cycle-column-width|center-column|consume|expel|close>\n       feresectl <get-focused-window|get-workspaces|get-outputs>".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direction_and_workspace_commands() {
        assert_eq!(
            parse_args(["focus".to_owned(), "left".to_owned()])
                .unwrap()
                .1,
            json!({ "direction": "left" })
        );
        assert_eq!(
            parse_args(["workspace".to_owned(), "7".to_owned()])
                .unwrap()
                .1,
            json!({ "index": 7 })
        );
        assert_eq!(
            parse_args(["cycle-column-width".to_owned()]).unwrap(),
            ("cycle-column-width".to_owned(), json!({}))
        );
    }

    #[test]
    fn rejects_unknown_or_missing_arguments() {
        assert!(parse_args(["focus".to_owned()]).is_err());
        assert!(parse_args(["reload-config".to_owned()]).is_err());
    }
}
