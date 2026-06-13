// cgx-fixture: async/await
// Covers: calls:async (await on async fn), suspends property at await sites,
//         async method on struct

use std::time::Duration;

pub async fn fetch_data(url: &str) -> Result<String, String> {
    let raw = http_get(url).await?;
    let parsed = parse_response(&raw).await?;
    Ok(parsed)
}

async fn http_get(url: &str) -> Result<String, String> {
    // Simulated HTTP call — in real code this would be reqwest or hyper
    if url.is_empty() {
        return Err("empty url".to_string());
    }
    Ok(format!("response from {}", url))
}

async fn parse_response(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        Err("empty response".to_string())
    } else {
        Ok(raw.to_uppercase())
    }
}

pub struct AsyncService {
    base_url: String,
}

impl AsyncService {
    pub fn new(base_url: &str) -> Self {
        AsyncService { base_url: base_url.to_string() }
    }

    pub async fn get(&self, path: &str) -> Result<String, String> {
        let url = format!("{}/{}", self.base_url, path);
        fetch_data(&url).await
    }

    pub async fn post(&self, path: &str, body: &str) -> Result<String, String> {
        let url = format!("{}/{}", self.base_url, path);
        http_post(&url, body).await
    }
}

async fn http_post(url: &str, body: &str) -> Result<String, String> {
    if url.is_empty() || body.is_empty() {
        Err("invalid request".to_string())
    } else {
        Ok(format!("posted {} bytes to {}", body.len(), url))
    }
}

/// Awaiting multiple futures sequentially — each await is a suspension point.
pub async fn sequential_awaits(url: &str) -> String {
    let a = http_get(url).await.unwrap_or_default();
    let b = http_get(url).await.unwrap_or_default();
    format!("{}{}", a, b)
}

/// sleep-like simulation — suspension without a meaningful return value.
pub async fn delay_then_run(millis: u64) -> String {
    simulate_sleep(Duration::from_millis(millis)).await;
    "done".to_string()
}

async fn simulate_sleep(_d: Duration) {
    // placeholder — would be tokio::time::sleep in real code
}
