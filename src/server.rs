use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Request, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE,
        },
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::JoinSet;
use tokio::time::{Duration, sleep};

use crate::{
    audio::AudioController,
    auth::{self, AuthStore},
    browser::BrowserHandle,
    config::Config,
    model::{BrowserAction, RemoteState},
    network, setup, youtube,
};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLE_CSS: &str = include_str!("../assets/style.css");
const SETUP_HTML: &str = include_str!("../assets/setup.html");
const SETUP_JS: &str = include_str!("../assets/setup.js");
const SETUP_CSS: &str = include_str!("../assets/setup.css");

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub auth: Arc<AuthStore>,
    pub browser: BrowserHandle,
    pub audio: AudioController,
}

impl AppState {
    pub async fn build(config: Config) -> Result<Arc<Self>> {
        config.ensure_directories().await?;
        let auth = Arc::new(AuthStore::load(config.sessions_path()).await?);
        let browser = BrowserHandle::start(crate::browser::BrowserConfig {
            binary: config.browser.clone(),
            profile_dir: config.profile_dir.clone(),
        })
        .await;
        Ok(Arc::new(Self {
            config,
            auth,
            browser,
            audio: AudioController::new(),
        }))
    }

    pub async fn snapshot(&self) -> RemoteState {
        RemoteState {
            browser: self.browser.snapshot().await,
            volume: self.audio.status().await,
        }
    }
}

pub async fn run(state: Arc<AppState>, open_setup: bool) -> Result<()> {
    let router = build_router(state.clone());
    let mut servers = JoinSet::new();
    let mut bound = 0usize;

    for address in network::listener_addresses(&state.config) {
        match TcpListener::bind(address).await {
            Ok(listener) => {
                println!("CouchMote listening at http://{address}");
                let app = router.clone();
                servers.spawn(async move {
                    axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .await
                    .context("HTTP listener stopped")
                });
                bound += 1;
            }
            Err(error) => {
                tracing::warn!(%address, error = %error, "could not bind CouchMote listener");
            }
        }
    }

    if bound == 0 {
        return Err(anyhow!("CouchMote could not bind any HTTP listener"));
    }

    if open_setup {
        let url = format!("http://127.0.0.1:{}/setup", state.config.port);
        tokio::spawn(async move {
            sleep(Duration::from_millis(700)).await;
            match Command::new("xdg-open").arg(&url).status().await {
                Ok(status) if status.success() => {
                    tracing::debug!(%url, "opened CouchMote setup page")
                }
                Ok(status) => tracing::debug!(%url, ?status, "could not open CouchMote setup page"),
                Err(error) => tracing::debug!(%url, %error, "could not open CouchMote setup page"),
            }
        });
    }

    if let Ok(addresses) = network::tailnet_addresses() {
        if addresses.is_empty() && state.config.listen == crate::config::ListenMode::Tailnet {
            tracing::warn!(
                "no Tailscale address was found; the phone will not be able to connect until Tailscale is up"
            );
        }
    }

    tokio::signal::ctrl_c()
        .await
        .context("failed to wait for shutdown signal")?;
    servers.abort_all();
    while servers.join_next().await.is_some() {}
    Ok(())
}

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/setup", get(setup_page))
        .route("/assets/app.js", get(app_js))
        .route("/assets/style.css", get(style_css))
        .route("/assets/setup.js", get(setup_js))
        .route("/assets/setup.css", get(setup_css))
        .route("/healthz", get(healthz))
        .route("/api/setup/status", get(api_setup_status))
        .route("/api/setup/pair", post(api_setup_pair))
        .route("/api/setup/finish", post(api_setup_finish))
        .route("/api/state", get(api_state))
        .route("/api/pair", post(api_pair))
        .route("/api/search", post(api_search))
        .route("/api/search/open", post(api_search_open))
        .route("/api/youtube/open", post(api_youtube_open))
        .route("/api/action", post(api_action))
        .route("/api/session/logout", post(api_logout))
        .layer(DefaultBodyLimit::max(32 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), network_guard))
        .with_state(state)
}

async fn network_guard(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(connect_info) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return error_response(StatusCode::FORBIDDEN, "request source is unavailable");
    };
    if !state.config.allowed_remote(connect_info.0.ip()) {
        return error_response(
            StatusCode::FORBIDDEN,
            "CouchMote only accepts loopback or Tailscale clients",
        );
    }
    next.run(request).await
}

async fn index() -> Response {
    let mut response = HtmlResponse(INDEX_HTML).into_response();
    add_static_headers(&mut response, "text/html; charset=utf-8");
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self' data:")
    );
    response
}

async fn setup_page(ConnectInfo(address): ConnectInfo<SocketAddr>) -> Response {
    if !address.ip().is_loopback() {
        return error_response(
            StatusCode::FORBIDDEN,
            "the CouchMote setup page is only available on the TV box",
        );
    }
    let mut response = HtmlResponse(SETUP_HTML).into_response();
    add_static_headers(&mut response, "text/html; charset=utf-8");
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self' data:",
        ),
    );
    response
}

async fn app_js() -> Response {
    let mut response = HtmlResponse(APP_JS).into_response();
    add_static_headers(&mut response, "text/javascript; charset=utf-8");
    response
}

async fn style_css() -> Response {
    let mut response = HtmlResponse(STYLE_CSS).into_response();
    add_static_headers(&mut response, "text/css; charset=utf-8");
    response
}

async fn setup_js(ConnectInfo(address): ConnectInfo<SocketAddr>) -> Response {
    if !address.ip().is_loopback() {
        return error_response(
            StatusCode::FORBIDDEN,
            "the CouchMote setup page is only available on the TV box",
        );
    }
    let mut response = HtmlResponse(SETUP_JS).into_response();
    add_static_headers(&mut response, "text/javascript; charset=utf-8");
    response
}

async fn setup_css(ConnectInfo(address): ConnectInfo<SocketAddr>) -> Response {
    if !address.ip().is_loopback() {
        return error_response(
            StatusCode::FORBIDDEN,
            "the CouchMote setup page is only available on the TV box",
        );
    }
    let mut response = HtmlResponse(SETUP_CSS).into_response();
    add_static_headers(&mut response, "text/css; charset=utf-8");
    response
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"ok": true}))
}

async fn api_setup_status(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Response {
    if !is_local_setup_request(address) {
        return setup_forbidden();
    }
    match setup::status(&state.config, &state.browser.snapshot().await).await {
        Ok(status) => Json(status).into_response(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn api_setup_pair(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Response {
    if !is_local_setup_request(address) {
        return setup_forbidden();
    }
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin setup requests are not allowed",
        );
    }
    let pairing = match state.auth.issue_pairing_code().await {
        Ok(pairing) => pairing,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let urls = network::tailnet_addresses()
        .unwrap_or_default()
        .into_iter()
        .map(|address| match address {
            std::net::IpAddr::V4(address) => format!("http://{address}:{}", state.config.port),
            std::net::IpAddr::V6(address) => format!("http://[{address}]:{}", state.config.port),
        })
        .collect::<Vec<_>>();
    Json(json!({
        "code": pairing.code,
        "expires_at": pairing.expires_at,
        "tailnet_urls": urls,
    }))
    .into_response()
}

async fn api_setup_finish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<SetupFinishRequest>,
) -> Response {
    if !is_local_setup_request(address) {
        return setup_forbidden();
    }
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin setup requests are not allowed",
        );
    }

    let status = match setup::status(&state.config, &state.browser.snapshot().await).await {
        Ok(status) => status,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    if !status.can_finish {
        return error_response(
            StatusCode::PRECONDITION_FAILED,
            "finish the required setup checks first",
        );
    }
    if let Err(error) = setup::set_autostart(&state.config, request.autostart).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
    }
    if let Err(error) = state.config.mark_setup_complete().await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
    }
    Json(json!({
        "complete": true,
        "autostart": request.autostart,
    }))
    .into_response()
}

async fn api_state(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }
    Json(state.snapshot().await).into_response()
}

async fn api_pair(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<PairRequest>,
) -> Response {
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin control requests are not allowed",
        );
    }
    if headers.get(COOKIE).is_some() && require_session(&state, &headers).await.is_ok() {
        return Json(json!({"paired": true})).into_response();
    }

    match state
        .auth
        .consume_pairing(address.ip(), &request.code)
        .await
    {
        Ok(token) => {
            let mut response = Json(json!({"paired": true})).into_response();
            if let Ok(value) = HeaderValue::from_str(&auth::session_cookie(&token)) {
                response.headers_mut().insert(SET_COOKIE, value);
            }
            response
        }
        Err(error) => error_response(StatusCode::UNAUTHORIZED, &error.to_string()),
    }
}

async fn api_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Response {
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin control requests are not allowed",
        );
    }
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }
    if request.query.trim().is_empty() || request.query.chars().count() > 200 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "search query must contain 1 to 200 characters",
        );
    }
    match state.browser.search(request.query).await {
        Ok(()) => Json(state.snapshot().await).into_response(),
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

async fn api_search_open(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<OpenSearchRequest>,
) -> Response {
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin control requests are not allowed",
        );
    }
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }
    let Some(url) = state.browser.resolve_result(&request.id).await else {
        return error_response(
            StatusCode::NOT_FOUND,
            "search result is no longer available",
        );
    };
    match state.browser.open_url(url).await {
        Ok(()) => Json(state.snapshot().await).into_response(),
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

async fn api_youtube_open(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<OpenYoutubeRequest>,
) -> Response {
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin control requests are not allowed",
        );
    }
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }
    let url = match youtube::validate_youtube_watch_url(&request.url) {
        Ok(url) => url,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match state.browser.open_url(url).await {
        Ok(()) => Json(state.snapshot().await).into_response(),
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

async fn api_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ActionRequest>,
) -> Response {
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin control requests are not allowed",
        );
    }
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }

    let result = match request {
        ActionRequest::SetVolume { percent } => state.audio.set_volume(percent).await,
        ActionRequest::ToggleMute => state.audio.toggle_mute().await,
        request => state.browser.action(request.into_browser_action()).await,
    };
    match result {
        Ok(()) => Json(state.snapshot().await).into_response(),
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
    }
}

async fn api_logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross-origin control requests are not allowed",
        );
    }
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }
    let mut response = Json(json!({"logged_out": true})).into_response();
    if let Ok(value) = HeaderValue::from_str(&auth::clear_session_cookie()) {
        response.headers_mut().insert(SET_COOKIE, value);
    }
    response
}

async fn require_session(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let token = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| auth::session_from_cookie(Some(value)));
    if let Some(token) = token {
        if state.auth.authenticate(token).await {
            return Ok(());
        }
    }
    Err(error_response(
        StatusCode::UNAUTHORIZED,
        "pair this browser before controlling CouchMote",
    ))
}

fn is_local_setup_request(address: SocketAddr) -> bool {
    address.ip().is_loopback()
}

fn setup_forbidden() -> Response {
    error_response(
        StatusCode::FORBIDDEN,
        "the CouchMote setup page is only available on the TV box",
    )
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let http_origin = format!("http://{host}");
    let https_origin = format!("https://{host}");
    origin == http_origin || origin == https_origin
}

fn add_static_headers(response: &mut Response, content_type: &'static str) {
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

struct HtmlResponse(&'static str);

impl IntoResponse for HtmlResponse {
    fn into_response(self) -> Response {
        Response::new(self.0.to_string().into())
    }
}

#[derive(Debug, Deserialize)]
struct PairRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
    query: String,
}

#[derive(Debug, Deserialize)]
struct OpenSearchRequest {
    id: String,
}

#[derive(Debug, Deserialize)]
struct OpenYoutubeRequest {
    url: String,
}

#[derive(Debug, Deserialize)]
struct SetupFinishRequest {
    #[serde(default = "default_true")]
    autostart: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ActionRequest {
    Launch,
    PlayPause,
    Seek { seconds: i16 },
    Next,
    Previous,
    Fullscreen,
    Back,
    Home,
    Navigate { direction: NavigationDirection },
    SetVolume { percent: u8 },
    ToggleMute,
}

impl ActionRequest {
    fn into_browser_action(self) -> BrowserAction {
        match self {
            Self::Launch => BrowserAction::Launch,
            Self::PlayPause => BrowserAction::PlayPause,
            Self::Seek { seconds } => BrowserAction::Seek {
                seconds: seconds.clamp(-60, 60),
            },
            Self::Next => BrowserAction::Next,
            Self::Previous => BrowserAction::Previous,
            Self::Fullscreen => BrowserAction::Fullscreen,
            Self::Back => BrowserAction::Back,
            Self::Home => BrowserAction::Home,
            Self::Navigate { direction } => match direction {
                NavigationDirection::Up => BrowserAction::NavigateUp,
                NavigationDirection::Down => BrowserAction::NavigateDown,
                NavigationDirection::Left => BrowserAction::NavigateLeft,
                NavigationDirection::Right => BrowserAction::NavigateRight,
                NavigationDirection::Select => BrowserAction::NavigateSelect,
            },
            Self::SetVolume { .. } | Self::ToggleMute => unreachable!(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NavigationDirection {
    Up,
    Down,
    Left,
    Right,
    Select,
}
