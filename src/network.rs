pub(crate) fn retry<T, E>(mut request: impl FnMut() -> Result<T, E>) -> Result<T, E> {
    for attempt in 0..3 {
        match request() {
            Ok(value) => return Ok(value),
            Err(_) if attempt < 2 => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn retries_transient_failures_and_returns_the_last_error() {
        let mut attempts = 0;
        assert_eq!(
            super::retry(|| {
                attempts += 1;
                (attempts == 3).then_some("ok").ok_or("disconnected")
            }),
            Ok("ok")
        );
        assert_eq!(attempts, 3);

        assert_eq!(
            super::retry::<(), _>(|| Err("still down")),
            Err("still down")
        );
    }
}
