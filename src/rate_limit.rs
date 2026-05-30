use axum::{extract::Request, http::{HeaderMap, StatusCode}, middleware::Next, response::{IntoResponse, Response}};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::{net::IpAddr, num::NonZeroU32, sync::Arc};

pub type KeyedLimiter = Arc<DefaultKeyedRateLimiter<IpAddr>>;

pub fn new_limiter(per_minute: u32, burst: u32) -> KeyedLimiter {
    Arc::new(RateLimiter::keyed(
        Quota::per_minute(NonZeroU32::new(per_minute).unwrap())
            .allow_burst(NonZeroU32::new(burst).unwrap()),
    ))
}

pub fn client_ip(headers: &HeaderMap) -> IpAddr {
    headers
        .get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .and_then(|s| s.parse().ok())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

pub async fn enforce(lim: KeyedLimiter, req: Request, next: Next) -> Response {
    if lim.check_key(&client_ip(req.headers())).is_ok() {
        next.run(req).await
    } else {
        StatusCode::TOO_MANY_REQUESTS.into_response()
    }
}
