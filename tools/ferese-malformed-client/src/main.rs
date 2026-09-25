use std::env;
use std::error::Error;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const UNKNOWN_OBJECT_FRAME: [u8; 8] = frame_header(u32::MAX, 0, 8);
const UNDERSIZED_FRAME: [u8; 8] = frame_header(1, 0, 4);
const UNALIGNED_FRAME: [u8; 9] = {
    let header = frame_header(1, 0, 9);
    [
        header[0], header[1], header[2], header[3], header[4], header[5], header[6], header[7], 0,
    ]
};

struct MalformedCase {
    name: &'static str,
    bytes: &'static [u8],
}

fn main() -> Result<(), Box<dyn Error>> {
    let socket = display_socket()?;
    let cases = malformed_cases();

    for case in cases {
        exercise_case(&socket, &case)?;
        println!("PASS {}", case.name);
    }

    Ok(())
}

fn display_socket() -> Result<PathBuf, io::Error> {
    let display = env::var_os("WAYLAND_DISPLAY")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "WAYLAND_DISPLAY is not set"))?;
    let display = PathBuf::from(display);
    if display.is_absolute() {
        return Ok(display);
    }

    env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|directory| directory.join(display))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))
}

fn malformed_cases() -> [MalformedCase; 3] {
    [
        MalformedCase {
            name: "unknown-object",
            bytes: &UNKNOWN_OBJECT_FRAME,
        },
        MalformedCase {
            name: "undersized-frame",
            bytes: &UNDERSIZED_FRAME,
        },
        MalformedCase {
            name: "unaligned-frame",
            bytes: &UNALIGNED_FRAME,
        },
    ]
}

const fn frame_header(object: u32, opcode: u16, size: u16) -> [u8; 8] {
    let object = object.to_ne_bytes();
    let size_and_opcode = ((size as u32) << 16 | opcode as u32).to_ne_bytes();
    [
        object[0],
        object[1],
        object[2],
        object[3],
        size_and_opcode[0],
        size_and_opcode[1],
        size_and_opcode[2],
        size_and_opcode[3],
    ]
}

fn exercise_case(socket: &PathBuf, case: &MalformedCase) -> Result<(), Box<dyn Error>> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(DISCONNECT_TIMEOUT))?;
    stream.write_all(case.bytes)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = [0_u8; 256];
    loop {
        match stream.read(&mut response) {
            Ok(0) => return Ok(()),
            Ok(_) => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(format!(
                    "compositor did not disconnect malformed case {:?} within {:?}",
                    case.name, DISCONNECT_TIMEOUT
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_native_wayland_headers() {
        let header = UNKNOWN_OBJECT_FRAME;

        assert_eq!(
            u32::from_ne_bytes(header[..4].try_into().unwrap()),
            u32::MAX
        );
        assert_eq!(u32::from_ne_bytes(header[4..].try_into().unwrap()), 8 << 16);
    }

    #[test]
    fn malformed_sizes_match_the_transmitted_byte_counts() {
        let undersized = UNDERSIZED_FRAME;
        let unaligned = UNALIGNED_FRAME;

        assert_eq!(
            u32::from_ne_bytes(undersized[4..].try_into().unwrap()) >> 16,
            4
        );
        assert_eq!(
            u32::from_ne_bytes(unaligned[4..8].try_into().unwrap()) >> 16,
            9
        );
        assert_eq!(unaligned.len(), 9);
    }
}
