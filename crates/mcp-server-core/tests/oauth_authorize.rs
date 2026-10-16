mod oauth_support;
use axum::http::StatusCode;
use mcp_server_core::jwt::{Payload, TokenKind};
use oauth_support::*;

const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

fn client_id(st: &mcp_server_core::oauth::AuthState) -> String {
    st.jwt
        .issue(
            TokenKind::Client,
            None,
            Payload {
                redirect_uris: Some(vec!["https://app.example.com/cb".into()]),
                ..Default::default()
            },
        )
        .unwrap()
}

fn query(client_id: &str, redirect: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
            ("state", "st<ate>"),
        ])
        .finish()
}

fn extract_hidden(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html.find(&marker).unwrap() + marker.len();
    let end = html[start..].find('"').unwrap() + start;
    html[start..end]
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn form_fields(html: &str, password: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = [
        "response_type",
        "client_id",
        "redirect_uri",
        "code_challenge",
        "code_challenge_method",
        "state",
        "scope",
        "resource",
        "csrf",
    ]
    .iter()
    .map(|n| (n.to_string(), extract_hidden(html, n)))
    .collect();
    v.push(("password".into(), password.into()));
    v
}

async fn login_page(app: &axum::Router, cid: &str) -> String {
    let res = send(
        app,
        get(&format!(
            "/authorize?{}",
            query(cid, "https://app.example.com/cb")
        )),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["x-frame-options"], "DENY");
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert!(
        res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("default-src 'none'")
    );
    assert!(
        !res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("form-action")
    );
    body_text(res).await
}

fn post(fields: &[(String, String)]) -> axum::http::Request<axum::body::Body> {
    let pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    form_post("/authorize", &pairs)
}

#[tokio::test]
async fn login_page_escapes_parameters() {
    let st = state();
    let html = login_page(&app(st.clone()), &client_id(&st)).await;
    assert!(html.contains("st&lt;ate&gt;"));
    assert!(!html.contains("st<ate>"));
    assert!(html.contains("<form method=\"post\" action=\"/authorize\">"));
}

#[tokio::test]
async fn correct_password_redirects_with_code_and_state() {
    let st = state();
    let app = app(st.clone());
    let html = login_page(&app, &client_id(&st)).await;
    let res = send(&app, post(&form_fields(&html, PASSWORD))).await;
    assert_eq!(res.status(), StatusCode::FOUND);
    let loc = url::Url::parse(res.headers()["location"].to_str().unwrap()).unwrap();
    assert_eq!(loc.host_str(), Some("app.example.com"));
    let q: std::collections::HashMap<_, _> = loc.query_pairs().into_owned().collect();
    assert_eq!(q["state"], "st<ate>");
    let code = st.jwt.verify(&q["code"], TokenKind::Code).unwrap();
    assert_eq!(code.payload.code_challenge.as_deref(), Some(CHALLENGE));
    assert_eq!(
        code.payload.redirect_uri.as_deref(),
        Some("https://app.example.com/cb")
    );
    assert!(code.exp.unwrap() - code.iat <= 60);
}

#[tokio::test]
async fn wrong_password_rerenders_with_401() {
    let st = state();
    let app = app(st.clone());
    let html = login_page(&app, &client_id(&st)).await;
    let res = send(&app, post(&form_fields(&html, "nope"))).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(body_text(res).await.contains("Incorrect password"));
}

#[tokio::test]
async fn sixth_attempt_is_rate_limited_even_with_correct_password() {
    let st = state();
    let app = app(st.clone());
    let html = login_page(&app, &client_id(&st)).await;
    for _ in 0..5 {
        assert_eq!(
            send(&app, post(&form_fields(&html, "nope"))).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(&app, post(&form_fields(&html, PASSWORD)))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn tampered_parameters_fail_csrf_binding() {
    let st = state();
    let app = app(st.clone());
    let html = login_page(&app, &client_id(&st)).await;
    let mut fields = form_fields(&html, PASSWORD);
    for f in fields.iter_mut() {
        if f.0 == "state" {
            f.1 = "different".into();
        }
    }
    assert_eq!(
        send(&app, post(&fields)).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn unregistered_redirect_uri_renders_400_and_never_redirects() {
    let st = state();
    let res = send(
        &app(st.clone()),
        get(&format!(
            "/authorize?{}",
            query(&client_id(&st), "https://evil.example.com/cb")
        )),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(res.headers().get("location").is_none());
}

#[tokio::test]
async fn invalid_method_redirects_with_error() {
    let st = state();
    let q = query(&client_id(&st), "https://app.example.com/cb")
        .replace("code_challenge_method=S256", "code_challenge_method=plain");
    let res = send(&app(st), get(&format!("/authorize?{q}"))).await;
    assert_eq!(res.status(), StatusCode::FOUND);
    let loc = res.headers()["location"].to_str().unwrap();
    assert!(loc.starts_with("https://app.example.com/cb?"));
    assert!(loc.contains("error=invalid_request"));
}
