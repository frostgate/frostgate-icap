use std::time::Duration;
use tokio::time::sleep;

pub async fn retry_async<F, Fut, T, E>(
    mut operation: F,
    max_retries: u32,
    base_delay: Duration,
    exponential: bool,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut last_error = None;
    for attempt in 0..=max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries {
                    let delay = if exponential {
                        base_delay * 2u32.pow(attempt)
                    } else {
                        base_delay
                    };
                    sleep(delay).await;
                }
            }
        }
    }
    Err(last_error.unwrap())
}