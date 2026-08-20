use std::time::Duration;

/// GitHub's release CDN can reset connections for a while after assets are
/// replaced; retries without delay all land in the same outage.
const RETRY_ATTEMPTS: u32 = 5;

#[cfg(feature = "network")]
pub(crate) fn retry<T>(request: impl FnMut() -> Result<T, ureq::Error>) -> Result<T, ureq::Error> {
    retry_with_sleep(std::thread::sleep, retryable_request, request)
}

fn retry_with_sleep<T, E>(
    mut sleep: impl FnMut(Duration),
    should_retry: impl Fn(&E) -> bool,
    mut request: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    for attempt in 0..RETRY_ATTEMPTS {
        match request() {
            Ok(value) => return Ok(value),
            Err(error) if attempt + 1 == RETRY_ATTEMPTS || !should_retry(&error) => {
                return Err(error);
            }
            Err(_) => sleep(backoff_after(attempt)),
        }
    }
    unreachable!()
}

#[cfg(feature = "network")]
fn retryable_request(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::StatusCode(408 | 429 | 500..=599)
            | ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
    )
}

fn backoff_after(attempt: u32) -> Duration {
    Duration::from_millis(match attempt {
        0 => 200,
        1 => 1_000,
        2 => 3_000,
        _ => 8_000,
    })
}

#[cfg(feature = "network")]
pub(crate) fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    agent().get(url).call()
}

#[cfg(feature = "network")]
fn agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .user_agent(user_agent())
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_global(Some(Duration::from_secs(120)))
                .build()
                .into()
        })
        .clone()
}

#[cfg(feature = "network")]
fn user_agent() -> String {
    format!("editur/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn retries_transient_failures_with_backoff_and_returns_the_last_error() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        assert_eq!(
            super::retry_with_sleep(
                |delay| delays.push(delay),
                |_| true,
                || {
                    attempts += 1;
                    (attempts == 5).then_some("ok").ok_or("disconnected")
                }
            ),
            Ok("ok")
        );
        assert_eq!(attempts, 5);
        assert_eq!(
            delays,
            [
                Duration::from_millis(200),
                Duration::from_secs(1),
                Duration::from_secs(3),
                Duration::from_secs(8),
            ]
        );

        attempts = 0;
        delays.clear();
        assert_eq!(
            super::retry_with_sleep::<(), _>(
                |delay| delays.push(delay),
                |_| true,
                || {
                    attempts += 1;
                    Err("still down")
                }
            ),
            Err("still down")
        );
        assert_eq!(attempts, 5);
        assert_eq!(delays.len(), 4);

        attempts = 0;
        delays.clear();
        assert_eq!(
            super::retry_with_sleep::<(), _>(
                |delay| delays.push(delay),
                |_| false,
                || {
                    attempts += 1;
                    Err("not found")
                }
            ),
            Err("not found")
        );
        assert_eq!(attempts, 1);
        assert!(delays.is_empty());
    }

    #[cfg(feature = "network")]
    #[test]
    fn retries_only_transient_http_statuses() {
        assert!(!super::retryable_request(&ureq::Error::StatusCode(404)));
        assert!(!super::retryable_request(&ureq::Error::StatusCode(403)));
        assert!(super::retryable_request(&ureq::Error::StatusCode(408)));
        assert!(super::retryable_request(&ureq::Error::StatusCode(429)));
        assert!(super::retryable_request(&ureq::Error::StatusCode(503)));
    }

    #[cfg(feature = "network")]
    #[test]
    fn downloads_identify_editur_to_the_release_host() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            request
        });

        let mut response =
            super::get(&format!("http://{addr}/editur-macos-aarch64.sha256")).unwrap();
        let body = response.body_mut().read_to_vec().unwrap();
        let request = server.join().unwrap();
        assert_eq!(body, b"ok");
        assert!(
            request
                .to_ascii_lowercase()
                .contains(&format!("user-agent: editur/{}", env!("CARGO_PKG_VERSION"))),
            "{request}"
        );
    }
}
