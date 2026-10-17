use crate::oauth::{AuthState, oauth_router, require_bearer};
use axum::Router;
use axum::extract::{DefaultBodyLimit, MatchedPath, Request};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use std::sync::Arc;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub const BODY_LIMIT: usize = 1024 * 1024;

pub async fn http_metrics(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let res = next.run(req).await;
    metrics::counter!("http_requests_total", "route" => route, "status" => res.status().as_u16().to_string()).increment(1);
    res
}

pub fn build_app(auth: Arc<AuthState>, mcp: Router) -> Router {
    let protected = mcp.layer(middleware::from_fn_with_state(auth.clone(), require_bearer));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(oauth_router(auth))
        .merge(protected)
        .layer(middleware::from_fn(http_metrics))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(TraceLayer::new_for_http().make_span_with(|req: &Request| {
            tracing::info_span!("request", method = %req.method(), path = req.uri().path())
        }))
}
