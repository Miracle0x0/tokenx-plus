use super::{emit_warning, PricingDiagnosticSink};

pub(super) const MAX_ATTEMPTS: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 200;

pub(super) fn build_client(
    builder: reqwest::ClientBuilder,
) -> Result<reqwest::Client, reqwest::Error> {
    builder
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
}

/// `None` advances the caller's attempt loop after a diagnosed, delayed retry.
pub(super) async fn request_attempt(
    client: &reqwest::Client,
    url: &str,
    source: &str,
    attempt: u32,
    diagnostics: &mut PricingDiagnosticSink<'_>,
) -> Result<Option<reqwest::Response>, reqwest::Error> {
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                return Ok(Some(response));
            }

            let retryable =
                status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            if retryable {
                emit_warning(
                    diagnostics,
                    format!(
                        "[tokenx] {source} HTTP {status} (attempt {}/{MAX_ATTEMPTS})",
                        attempt + 1,
                    ),
                );
            } else {
                emit_warning(diagnostics, format!("[tokenx] {source} HTTP {status}"));
            }
            if !retryable || attempt == MAX_ATTEMPTS - 1 {
                return Err(response.error_for_status().unwrap_err());
            }
            let _ = response.bytes().await;
        }
        Err(error) => {
            emit_warning(
                diagnostics,
                format!(
                    "[tokenx] {source} network error (attempt {}/{MAX_ATTEMPTS}): {error}",
                    attempt + 1,
                ),
            );
            if attempt == MAX_ATTEMPTS - 1 {
                return Err(error);
            }
        }
    }
    retry_delay(attempt).await;
    Ok(None)
}

pub(super) async fn retry_delay(attempt: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(
        INITIAL_BACKOFF_MS * (1 << attempt),
    ))
    .await;
}
