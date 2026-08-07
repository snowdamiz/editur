use crate::file_io::OpenTarget;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::mpsc::{self, Sender},
    time::Duration,
};
use winit::event_loop::EventLoopProxy;

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAGIC: &[u8; 8] = b"EDITUR1\0";
const ACKNOWLEDGED: &[u8; 2] = b"OK";
const REFUSED: &[u8; 2] = b"NO";

#[derive(serde::Deserialize, serde::Serialize)]
enum Request {
    Open(OpenTarget),
    Quit,
}

pub enum InstanceEvent {
    Open(OpenTarget, Sender<bool>),
    Quit(Sender<bool>),
    Wake,
    Exit,
}

pub enum Claim {
    Primary(TcpListener),
    Forwarded,
}

pub fn open_running(target: &OpenTarget) -> Result<bool, String> {
    match forward_request(instance_address()?, &Request::Open(target.clone()))? {
        Some(true) => Ok(true),
        Some(false) => Err("running editor refused the open request".into()),
        None => Ok(false),
    }
}

pub fn claim(target: &OpenTarget) -> Result<Claim, String> {
    let address = instance_address()?;
    if forward_request(address, &Request::Open(target.clone()))? == Some(true) {
        return Ok(Claim::Forwarded);
    }
    match TcpListener::bind(address) {
        Ok(listener) => Ok(Claim::Primary(listener)),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            match forward_request(address, &Request::Open(target.clone()))? {
                Some(true) => Ok(Claim::Forwarded),
                _ => Err("another process is using Editur's local instance port".into()),
            }
        }
        Err(error) => Err(format!(
            "cannot start local editor instance listener: {error}"
        )),
    }
}

pub fn spawn_listener(
    listener: TcpListener,
    proxy: EventLoopProxy<InstanceEvent>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("editur-instance-listener".into())
        .spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut stream) = connection else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                match receive_request(&mut stream) {
                    Ok(Request::Open(target)) => {
                        let (reply, response) = mpsc::channel();
                        let accepted = proxy.send_event(InstanceEvent::Open(target, reply)).is_ok()
                            && response.recv_timeout(Duration::from_secs(2)) == Ok(true);
                        let _ = send_response(&mut stream, accepted);
                        if !accepted {
                            break;
                        }
                    }
                    Ok(Request::Quit) => {
                        let (reply, response) = mpsc::channel();
                        let accepted = proxy.send_event(InstanceEvent::Quit(reply)).is_ok()
                            && response.recv_timeout(Duration::from_secs(2)) == Ok(true);
                        let _ = send_response(&mut stream, accepted);
                        if accepted {
                            let _ = proxy.send_event(InstanceEvent::Exit);
                            break;
                        }
                    }
                    Err(_) => {}
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("cannot start local editor instance listener: {error}"))
}

fn instance_address() -> Result<SocketAddr, String> {
    let data_dir = crate::syntax::data_dir()?;
    Ok(SocketAddr::from(([127, 0, 0, 1], instance_port(&data_dir))))
}

fn instance_port(data_dir: &Path) -> u16 {
    let hash = data_dir
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        });
    49_152 + (hash % 10_000) as u16
}

pub fn quit_running() -> Result<bool, String> {
    Ok(forward_request(instance_address()?, &Request::Quit)?.unwrap_or(true))
}

fn forward_request(address: SocketAddr, request: &Request) -> Result<Option<bool>, String> {
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(75)) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(format!("cannot connect to running editor: {error}")),
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(2))))
        .map_err(|error| format!("cannot configure running editor connection: {error}"))?;
    let message = serde_json::to_vec(request)
        .map_err(|error| format!("cannot encode editor open request: {error}"))?;
    if message.len() > MAX_MESSAGE_BYTES {
        return Err("editor open request is too large".into());
    }
    stream
        .write_all(MAGIC)
        .and_then(|()| stream.write_all(&(message.len() as u32).to_be_bytes()))
        .and_then(|()| stream.write_all(&message))
        .map_err(|error| format!("cannot notify running editor: {error}"))?;
    let mut ack = [0; ACKNOWLEDGED.len()];
    stream
        .read_exact(&mut ack)
        .map_err(|error| format!("running editor did not acknowledge request: {error}"))?;
    match &ack {
        ACKNOWLEDGED => Ok(Some(true)),
        REFUSED => Ok(Some(false)),
        _ => Err("running editor returned an invalid acknowledgement".into()),
    }
}

fn receive_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut magic = [0; MAGIC.len()];
    stream
        .read_exact(&mut magic)
        .map_err(|error| format!("cannot read editor request header: {error}"))?;
    if magic != *MAGIC {
        return Err("invalid editor request header".into());
    }
    let mut length = [0; 4];
    stream
        .read_exact(&mut length)
        .map_err(|error| format!("cannot read editor open request: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE_BYTES {
        return Err("editor open request is too large".into());
    }
    let mut message = vec![0; length];
    stream
        .read_exact(&mut message)
        .map_err(|error| format!("cannot read editor open request: {error}"))?;
    serde_json::from_slice(&message)
        .map_err(|error| format!("cannot decode editor open request: {error}"))
}

fn send_response(stream: &mut TcpStream, accepted: bool) -> Result<(), String> {
    stream
        .write_all(if accepted { ACKNOWLEDGED } else { REFUSED })
        .map_err(|error| format!("cannot acknowledge editor request: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{Request, forward_request, instance_port, receive_request, send_response};
    use crate::file_io::OpenTarget;
    use std::path::PathBuf;
    use std::{io::Write, net::TcpListener};

    #[test]
    fn instance_port_is_stable_across_releases() {
        assert_eq!(instance_port(std::path::Path::new("user-data")), 56_381);
    }

    #[test]
    fn forwards_an_open_target_to_the_running_instance() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let target = OpenTarget {
            root: PathBuf::from("project"),
            file: Some(PathBuf::from("project/main.rs")),
            create: false,
        };
        let expected = target.clone();
        let receiver = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = receive_request(&mut stream).unwrap();
            send_response(&mut stream, true).unwrap();
            request
        });

        assert_eq!(
            forward_request(address, &Request::Open(target)).unwrap(),
            Some(true)
        );

        let Request::Open(received) = receiver.join().unwrap() else {
            panic!("expected open request")
        };
        assert_eq!(received, expected);
    }

    #[test]
    fn rejects_connections_that_are_not_editur_instances() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let sender = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.write_all(b"NOTEDITR").unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();

        assert!(receive_request(&mut stream).is_err());
        sender.join().unwrap();
    }
}
