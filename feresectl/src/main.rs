use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
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
        command: command.clone(),
        args,
    };
    let mut stream = UnixStream::connect(socket_path()?)?;

    write_frame(&mut stream, &request)?;
    let response: Response = read_frame(&mut stream)?;
    if let Some(error) = response.error {
        return Err(format!("{}: {}", error.code, error.message).into());
    }

    if command == "screenshot" {
        write_png(&response)
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&response.result.unwrap_or(Value::Null))?
        );
        Ok(())
    }
}

// The compositor stages the PNG in a private directory and replies with its
// path. Open it before unlinking so a failure never destroys the only copy, and
// stream the bytes with explicit writes: println! would corrupt binary output.
fn write_png(response: &Response) -> Result<(), Box<dyn Error>> {
    let path = response
        .result
        .as_ref()
        .and_then(|result| result.get("path"))
        .and_then(Value::as_str)
        .ok_or("the compositor did not return a screenshot path")?;
    let mut file = fs::File::open(path)?;
    if let Err(error) = fs::remove_file(path) {
        // The compositor sweeps anything left behind, so a failure here is not
        // worth losing the capture over.
        eprintln!("feresectl: could not remove {path}: {error}");
    }
    let mut stdout = io::stdout().lock();
    io::copy(&mut file, &mut stdout)?;
    stdout.flush()?;
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
        "screenshot" => match positional.as_slice() {
            [] => json!({}),
            [flag, geometry] if flag == "--geometry" || flag == "-g" => {
                json!({ "geometry": geometry })
            }
            _ => {
                return Err(format!(
                    "{command} accepts an optional --geometry \"x,y WxH\" ({})",
                    usage()
                ));
            }
        },
        "toggle-floating" | "toggle-fullscreen" | "toggle-maximized" | "toggle-layout"
        | "toggle-overview" | "cycle-column-width" | "center-column" | "consume" | "expel"
        | "close" | "get-focused-window" | "get-workspaces" | "get-outputs" | "reload-config"
        | "exit" => {
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
    "usage: feresectl screenshot [--geometry \"x,y WxH\"]\n       feresectl <focus|move|resize> <direction>\n       feresectl <workspace|move-to-workspace> <index>\n       feresectl <toggle-floating|toggle-maximized|toggle-fullscreen|toggle-layout|toggle-overview>\n       feresectl <cycle-column-width|center-column|consume|expel|close|exit>\n       feresectl <get-focused-window|get-workspaces|get-outputs|reload-config>".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_toggle_accepts_no_arguments() {
        assert_eq!(
            parse_args(["toggle-overview".into()]).unwrap(),
            ("toggle-overview".into(), json!({}))
        );
        assert!(parse_args(["toggle-overview".into(), "extra".into()]).is_err());
    }

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
    fn screenshot_takes_an_optional_geometry() {
        assert_eq!(
            parse_args(["screenshot".into()]).unwrap(),
            ("screenshot".into(), json!({})),
            "no geometry captures every enabled output"
        );
        assert_eq!(
            parse_args(["screenshot".into(), "-g".into(), "10,-20 300x200".into()])
                .unwrap()
                .1,
            json!({ "geometry": "10,-20 300x200" }),
            "negative origins survive argument parsing"
        );
        assert_eq!(
            parse_args(["screenshot".into(), "--geometry".into(), "0,0 8x8".into()])
                .unwrap()
                .1,
            json!({ "geometry": "0,0 8x8" })
        );
    }

    #[test]
    fn screenshot_rejects_stray_arguments() {
        assert!(parse_args(["screenshot".into(), "extra".into()]).is_err());
        assert!(parse_args(["screenshot".into(), "-g".into()]).is_err());
        assert!(
            parse_args(["screenshot".into(), "0,0 8x8".into()]).is_err(),
            "a bare geometry is not accepted without the flag"
        );
    }

    #[test]
    fn rejects_unknown_or_missing_arguments() {
        assert_eq!(
            parse_args(["exit".to_owned()]).unwrap(),
            ("exit".to_owned(), json!({}))
        );
        assert!(parse_args(["exit".to_owned(), "extra".to_owned()]).is_err());
        assert!(parse_args(["focus".to_owned()]).is_err());
        assert!(parse_args(["reload-config".to_owned()]).is_ok());
        assert!(parse_args(["reload-config".to_owned(), "extra".to_owned()]).is_err());
    }
}
