use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::{
    env,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;
use wol_core::{Device, MacAddress};

const SESSION_COOKIE: &str = "wolui_session";
const SESSION_SECONDS: i64 = 12 * 60 * 60;

#[derive(Serialize)]
struct ApiError {
    code: &'static str,
    message: String,
}

#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    site_id: String,
    access_digest: [u8; 32],
    secure_cookie: bool,
    public_origin: Option<String>,
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            code,
            message: message.into(),
        }),
    )
        .into_response()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}
fn random_secret() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

#[derive(Deserialize)]
struct LoginInput {
    token: String,
}
#[derive(Deserialize, Serialize)]
struct CsrfOutput {
    csrf_token: String,
}
#[derive(Serialize)]
struct Site {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct DeviceInput {
    name: String,
    mac: String,
    network_profile_id: String,
    tailscale_name: Option<String>,
}
#[derive(Deserialize)]
struct DevicePatch {
    name: Option<String>,
    mac: Option<String>,
    network_profile_id: Option<String>,
    tailscale_name: Option<Option<String>>,
    revision: i64,
}
#[derive(Deserialize, Serialize)]
struct WakeOutput {
    id: String,
    status: String,
}

async fn current_token(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("wolui_session=").map(str::to_owned))
}
async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), Response> {
    let token = current_token(headers).await.ok_or_else(|| {
        error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid session is required",
        )
    })?;
    let d = hex::encode(digest(&token));
    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT csrf_token, expires_at FROM sessions WHERE token_digest = ?",
    )
    .bind(d)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Session lookup failed",
        )
    })?;
    match row {
        Some((csrf, expires)) if expires > now() => Ok((token, csrf)),
        _ => Err(error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid session is required",
        )),
    }
}
fn check_origin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        let origin = origin.trim_end_matches('/');
        let accepted = state
            .public_origin
            .as_deref()
            .map(|expected| expected.trim_end_matches('/') == origin)
            .unwrap_or(false);
        if !accepted {
            return Err(error(
                StatusCode::FORBIDDEN,
                "origin_forbidden",
                "Request origin is not allowed",
            ));
        }
    }
    Ok(())
}
fn check_csrf(expected: &str, headers: &HeaderMap) -> bool {
    headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|v| expected.as_bytes().ct_eq(v.as_bytes()).into())
        .unwrap_or(false)
}

async fn live() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status":"ok"})))
}
async fn ready(State(state): State<AppState>) -> Response {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"status":"ok"}))).into_response(),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "not_ready",
            "Database is unavailable",
        ),
    }
}
async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> Response {
    if let Err(response) = check_origin(&state, &headers) {
        return response;
    }
    if !bool::from(state.access_digest.ct_eq(&digest(&input.token))) {
        return error(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "The access token is invalid",
        );
    }
    let session = random_secret();
    let csrf = random_secret();
    let result = sqlx::query(
        "INSERT INTO sessions(token_digest, csrf_token, created_at, expires_at) VALUES(?, ?, ?, ?)",
    )
    .bind(hex::encode(digest(&session)))
    .bind(&csrf)
    .bind(now())
    .bind(now() + SESSION_SECONDS)
    .execute(&state.db)
    .await;
    if result.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not create session",
        );
    }
    let cookie = format!(
        "{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECONDS}{}",
        if state.secure_cookie { "; Secure" } else { "" }
    );
    let mut response = Json(CsrfOutput { csrf_token: csrf }).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    response
}
async fn current(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match require_session(&state, &headers).await {
        Ok((_, csrf)) => Json(CsrfOutput { csrf_token: csrf }).into_response(),
        Err(r) => r,
    }
}
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = check_origin(&state, &headers) {
        return response;
    }
    let (token, csrf) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !check_csrf(&csrf, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "A valid X-CSRF-Token is required",
        );
    }
    let _ = sqlx::query("DELETE FROM sessions WHERE token_digest = ?")
        .bind(hex::encode(digest(&token)))
        .execute(&state.db)
        .await;
    let cookie = format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if state.secure_cookie { "; Secure" } else { "" }
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    response
}

async fn sites(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    Json(vec![Site {
        id: state.site_id.clone(),
        name: "Local site".into(),
    }])
    .into_response()
}
async fn profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(site): Path<String>,
) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    match sqlx::query_as::<_, (String, String)>(
        "SELECT id,name FROM network_profiles ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|(id, name)| Site { id, name })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not load network profiles",
        ),
    }
}
async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(site): Path<String>,
) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    match fetch_devices(&state.db, None).await {
        Ok(items) => Json(items).into_response(),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not load devices",
        ),
    }
}
async fn fetch_devices(db: &SqlitePool, id: Option<&str>) -> Result<Vec<Device>, sqlx::Error> {
    let rows = if let Some(id) = id {
        sqlx::query_as::<_, (String, String, String, String, Option<String>, i64)>(
            "SELECT id,name,mac,network_profile_id,tailscale_name,revision FROM devices WHERE id=?",
        )
        .bind(id)
        .fetch_all(db)
        .await?
    } else {
        sqlx::query_as::<_,(String,String,String,String,Option<String>,i64)>("SELECT id,name,mac,network_profile_id,tailscale_name,revision FROM devices ORDER BY name").fetch_all(db).await?
    };
    Ok(rows
        .into_iter()
        .map(
            |(id, name, mac, network_profile_id, tailscale_name, revision)| Device {
                id,
                name,
                mac,
                network_profile_id,
                tailscale_name,
                revision,
            },
        )
        .collect())
}
fn validate_input(input: &DeviceInput) -> Result<(), (&'static str, &'static str)> {
    if input.name.trim().is_empty() || input.name.len() > 120 {
        return Err(("invalid_name", "Name must contain 1 to 120 characters"));
    }
    if input.mac.parse::<MacAddress>().is_err() {
        return Err(("invalid_mac", "MAC must be a nonzero unicast address"));
    }
    if input.network_profile_id.trim().is_empty() {
        return Err(("invalid_network_profile", "Network profile is required"));
    }
    if input.tailscale_name.as_ref().is_some_and(|s| s.len() > 253) {
        return Err(("invalid_tailscale_name", "Tailscale name is too long"));
    }
    Ok(())
}
async fn create_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(site): Path<String>,
    Json(input): Json<DeviceInput>,
) -> Response {
    if let Err(r) = check_origin(&state, &headers) {
        return r;
    }
    let (_, csrf) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !check_csrf(&csrf, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "A valid X-CSRF-Token is required",
        );
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    if let Err((code, message)) = validate_input(&input) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, code, message);
    }
    let id = Uuid::new_v4().to_string();
    let result = sqlx::query("INSERT INTO devices(id,name,mac,network_profile_id,tailscale_name,revision) SELECT ?,?,?,?, ?,1 WHERE EXISTS(SELECT 1 FROM network_profiles WHERE id=?)")
        .bind(&id).bind(input.name.trim()).bind(input.mac.to_ascii_lowercase()).bind(&input.network_profile_id).bind(&input.tailscale_name).bind(&input.network_profile_id).execute(&state.db).await;
    match result {
        Ok(r) if r.rows_affected() == 1 => match fetch_devices(&state.db, Some(&id)).await {
            Ok(mut d) => (StatusCode::CREATED, Json(d.remove(0))).into_response(),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load created device",
            ),
        },
        Ok(_) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_network_profile",
            "Network profile not found",
        ),
        Err(e) if e.to_string().contains("UNIQUE") => error(
            StatusCode::CONFLICT,
            "device_exists",
            "A device with this MAC already exists in the network profile",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not create device",
        ),
    }
}
async fn update_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((site, id)): Path<(String, String)>,
    Json(input): Json<DevicePatch>,
) -> Response {
    if let Err(r) = check_origin(&state, &headers) {
        return r;
    }
    let (_, csrf) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !check_csrf(&csrf, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "A valid X-CSRF-Token is required",
        );
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    if input.revision < 1 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_revision",
            "Revision must be positive",
        );
    }
    let old = match fetch_devices(&state.db, Some(&id)).await {
        Ok(mut v) => v.pop(),
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load device",
            )
        }
    };
    let Some(old) = old else {
        return error(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "Device not found",
        );
    };
    if old.revision != input.revision {
        return error(
            StatusCode::CONFLICT,
            "revision_conflict",
            "Device has changed; reload before editing",
        );
    }
    let candidate = DeviceInput {
        name: input.name.unwrap_or(old.name),
        mac: input.mac.unwrap_or(old.mac),
        network_profile_id: input.network_profile_id.unwrap_or(old.network_profile_id),
        tailscale_name: input.tailscale_name.unwrap_or(old.tailscale_name),
    };
    if let Err((code, msg)) = validate_input(&candidate) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, code, msg);
    }
    let result=sqlx::query("UPDATE devices SET name=?,mac=?,network_profile_id=?,tailscale_name=?,revision=revision+1 WHERE id=? AND revision=? AND EXISTS(SELECT 1 FROM network_profiles WHERE id=?)")
        .bind(candidate.name.trim()).bind(candidate.mac.to_ascii_lowercase()).bind(&candidate.network_profile_id).bind(candidate.tailscale_name).bind(&id).bind(input.revision).bind(&candidate.network_profile_id).execute(&state.db).await;
    match result {
        Ok(r) if r.rows_affected() == 1 => match fetch_devices(&state.db, Some(&id)).await {
            Ok(mut v) => Json(v.remove(0)).into_response(),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load updated device",
            ),
        },
        Ok(_) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_network_profile",
            "Network profile not found",
        ),
        Err(e) if e.to_string().contains("UNIQUE") => error(
            StatusCode::CONFLICT,
            "device_exists",
            "A device with this MAC already exists in the network profile",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not update device",
        ),
    }
}
async fn remove_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((site, id)): Path<(String, String)>,
) -> Response {
    if let Err(r) = check_origin(&state, &headers) {
        return r;
    }
    let (token, csrf) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let _ = token;
    if !check_csrf(&csrf, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "A valid X-CSRF-Token is required",
        );
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    match sqlx::query("DELETE FROM devices WHERE id=?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => error(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "Device not found",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not delete device",
        ),
    }
}
async fn wake(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((site, id)): Path<(String, String)>,
) -> Response {
    if let Err(r) = check_origin(&state, &headers) {
        return r;
    }
    let (_, csrf) = match require_session(&state, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !check_csrf(&csrf, &headers) {
        return error(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "A valid X-CSRF-Token is required",
        );
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    let Some(key) = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "idempotency_key_required",
            "A valid Idempotency-Key is required",
        );
    };
    if let Ok(Some((job, status, device_id))) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id,status,device_id FROM wake_jobs WHERE idempotency_key=?",
    )
    .bind(key)
    .fetch_optional(&state.db)
    .await
    {
        if device_id != id {
            return error(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "This Idempotency-Key was used for a different operation",
            );
        }
        return (StatusCode::ACCEPTED, Json(WakeOutput { id: job, status })).into_response();
    }
    let row=sqlx::query_as::<_,(String,String,String,String,u16)>("SELECT d.mac,p.bind_address,p.broadcast_address,p.interface,p.port FROM devices d JOIN network_profiles p ON p.id=d.network_profile_id WHERE d.id=?").bind(&id).fetch_optional(&state.db).await;
    let Some((mac, bind, broadcast, interface, port)) = (match row {
        Ok(v) => v,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load wake target",
            )
        }
    }) else {
        return error(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "Device not found",
        );
    };
    let job = Uuid::new_v4().to_string();
    if sqlx::query("INSERT INTO wake_jobs(id,idempotency_key,device_id,status,created_at) VALUES(?,?,?,'sending',?)").bind(&job).bind(key).bind(&id).bind(now()).execute(&state.db).await.is_err(){
        if let Ok(Some((existing,status,device_id)))=sqlx::query_as::<_,(String,String,String)>("SELECT id,status,device_id FROM wake_jobs WHERE idempotency_key=?").bind(key).fetch_optional(&state.db).await {
            if device_id != id { return error(StatusCode::CONFLICT,"idempotency_conflict","This Idempotency-Key was used for a different operation"); }
            return (StatusCode::ACCEPTED,Json(WakeOutput{id:existing,status})).into_response();
        }
        return error(StatusCode::CONFLICT,"idempotency_conflict","Could not reserve this wake request");
    }
    let status = match (
        mac.parse::<MacAddress>(),
        bind.parse::<Ipv4Addr>(),
        broadcast.parse::<Ipv4Addr>(),
    ) {
        (Ok(mac), Ok(bind), Ok(broadcast)) => send_magic_packet(mac, bind, broadcast, port)
            .map(|_| "sent")
            .unwrap_or("failed"),
        _ => "failed",
    };
    let _ = sqlx::query("UPDATE wake_jobs SET status=? WHERE id=?")
        .bind(status)
        .bind(&job)
        .execute(&state.db)
        .await;
    let _ = interface;
    (
        StatusCode::ACCEPTED,
        Json(WakeOutput {
            id: job,
            status: status.into(),
        }),
    )
        .into_response()
}

async fn wake_job_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((site, id)): Path<(String, String)>,
) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    if site != state.site_id {
        return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
    }
    match sqlx::query_as::<_, (String, String)>("SELECT id,status FROM wake_jobs WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some((id, status))) => Json(WakeOutput { id, status }).into_response(),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "wake_job_not_found",
            "Wake job not found",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not load wake job",
        ),
    }
}
fn send_magic_packet(
    mac: MacAddress,
    bind: Ipv4Addr,
    broadcast: Ipv4Addr,
    port: u16,
) -> std::io::Result<usize> {
    let socket = UdpSocket::bind(SocketAddr::from((bind, 0)))?;
    socket.set_broadcast(true)?;
    socket.send_to(&mac.magic_packet(), SocketAddr::from((broadcast, port)))
}

fn app(state: AppState, web_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/sessions", post(login))
        .route("/api/v1/sessions/current", get(current).delete(logout))
        .route("/api/v1/sites", get(sites))
        .route("/api/v1/sites/:site/network-profiles", get(profiles))
        .route(
            "/api/v1/sites/:site/devices",
            get(list_devices).post(create_device),
        )
        .route(
            "/api/v1/sites/:site/devices/:id",
            patch(update_device).delete(remove_device),
        )
        .route("/api/v1/sites/:site/devices/:id/wake", post(wake))
        .route("/api/v1/sites/:site/wake-jobs/:id", get(wake_job_status))
        .with_state(state);
    if let Some(path) = web_dir {
        api.fallback_service(
            ServeDir::new(path.clone())
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(path.join("index.html"))),
        )
        .layer(TraceLayer::new_for_http())
    } else {
        api.layer(TraceLayer::new_for_http())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    async fn test_state() -> (AppState, std::net::UdpSocket) {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&db).await.unwrap();
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let port = receiver.local_addr().unwrap().port();
        sqlx::query("INSERT INTO network_profiles(id,name,interface,bind_address,broadcast_address,port) VALUES('test','Test network','lo','127.0.0.1','127.0.0.1',?)")
            .bind(port).execute(&db).await.unwrap();
        (
            AppState {
                db,
                site_id: "local".into(),
                access_digest: digest("0123456789012345678901234567890123456789"),
                secure_cookie: false,
                public_origin: Some("http://localhost".into()),
            },
            receiver,
        )
    }

    #[tokio::test]
    async fn local_http_login_crud_and_wake_use_only_the_simulated_udp_receiver() {
        let (state, receiver) = test_state().await;
        let router = app(state, None);
        let live = router
            .clone()
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(live.status(), StatusCode::OK);
        let login = router
            .clone()
            .oneshot(
                Request::post("/api/v1/sessions")
                    .header("origin", "http://localhost")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"token":"0123456789012345678901234567890123456789"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let bytes = to_bytes(login.into_body(), usize::MAX).await.unwrap();
        let csrf: CsrfOutput = serde_json::from_slice(&bytes).unwrap();
        let current = router
            .clone()
            .oneshot(
                Request::get("/api/v1/sessions/current")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(current.status(), StatusCode::OK);
        let profiles = router
            .clone()
            .oneshot(
                Request::get("/api/v1/sites/local/network-profiles")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(profiles.status(), StatusCode::OK);
        let site_response = router
            .clone()
            .oneshot(
                Request::get("/api/v1/sites")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(site_response.status(), StatusCode::OK);
        let create = router.clone().oneshot(Request::post("/api/v1/sites/local/devices")
            .header(header::COOKIE, &cookie).header("x-csrf-token", &csrf.csrf_token)
            .header("origin", "http://localhost").header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Simulated","mac":"02:00:00:00:00:01","network_profile_id":"test","tailscale_name":null}"#)).unwrap()).await.unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let bytes = to_bytes(create.into_body(), usize::MAX).await.unwrap();
        let device: Device = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(device.revision, 1);
        let request = || {
            Request::post(format!("/api/v1/sites/local/devices/{}/wake", device.id))
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf.csrf_token)
                .header("origin", "http://localhost")
                .header("idempotency-key", "simulated-wake-1")
                .body(Body::empty())
                .unwrap()
        };
        let wake_response = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(wake_response.status(), StatusCode::ACCEPTED);
        let wake_bytes = to_bytes(wake_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let wake_job: WakeOutput = serde_json::from_slice(&wake_bytes).unwrap();
        let wake_status = router
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/sites/local/wake-jobs/{}", wake_job.id))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wake_status.status(), StatusCode::OK);
        let mut packet = [0u8; 102];
        let (size, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(size, 102);
        assert_eq!(&packet[..6], &[0xff; 6]);
        assert_eq!(&packet[6..12], &[2, 0, 0, 0, 0, 1]);
        let repeated = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(repeated.status(), StatusCode::ACCEPTED);
        assert!(receiver.set_nonblocking(true).is_ok());
        assert!(
            receiver.recv_from(&mut packet).is_err(),
            "idempotent retry must not send a second datagram"
        );
        let logout = router
            .clone()
            .oneshot(
                Request::delete("/api/v1/sessions/current")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &csrf.csrf_token)
                    .header("origin", "http://localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        let revoked = router
            .oneshot(
                Request::get("/api/v1/sessions/current")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }
}

#[derive(Deserialize)]
struct ProfileConfig {
    id: String,
    name: String,
    interface: String,
    bind_address: Ipv4Addr,
    broadcast_address: Ipv4Addr,
    #[serde(default = "default_port")]
    port: u16,
}
fn default_port() -> u16 {
    9
}
async fn db_pool(data_dir: &str) -> Result<SqlitePool, Box<dyn std::error::Error>> {
    tokio::fs::create_dir_all(data_dir).await?;
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{}/wolui.sqlite3", data_dir))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}
async fn load_profiles(db: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(json) = env::var("WOLUI_NETWORK_PROFILES") {
        let profiles: Vec<ProfileConfig> = serde_json::from_str(&json)?;
        for p in profiles {
            if p.id.trim().is_empty()
                || p.name.trim().is_empty()
                || p.interface.trim().is_empty()
                || p.port == 0
            {
                return Err("invalid WOLUI_NETWORK_PROFILES entry".into());
            }
            sqlx::query("INSERT INTO network_profiles(id,name,interface,bind_address,broadcast_address,port) VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,interface=excluded.interface,bind_address=excluded.bind_address,broadcast_address=excluded.broadcast_address,port=excluded.port")
                .bind(p.id).bind(p.name).bind(p.interface).bind(p.bind_address.to_string()).bind(p.broadcast_address.to_string()).bind(p.port).execute(db).await?;
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_filter = env::var("WOLUI_LOG").unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(log_filter)?)
        .init();
    let role = env::var("WOLUI_ROLE").unwrap_or_else(|_| "site".into());
    if role != "site" && role != "hub" {
        return Err("WOLUI_ROLE must be site or hub".into());
    }
    let allow_dev = env::args().any(|a| a == "--allow-loopback-dev");
    let listen = env::var("WOLUI_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let addr: SocketAddr = listen.parse()?;
    if allow_dev && !addr.ip().is_loopback() {
        return Err(
            "--allow-loopback-dev can only be used with a loopback WOLUI_LISTEN address".into(),
        );
    }
    if !allow_dev && addr.ip().is_loopback() {
        return Err("loopback listen requires --allow-loopback-dev".into());
    }
    let token_path = env::var("WOLUI_ACCESS_TOKEN_FILE")
        .map_err(|_| "WOLUI_ACCESS_TOKEN_FILE must point to the setup token file")?;
    let access_token = tokio::fs::read_to_string(token_path).await?;
    let access_token = access_token.trim();
    if access_token.len() < 40 {
        return Err("setup token must have at least 32 bytes of entropy".into());
    }
    let data = env::var("WOLUI_DATA_DIR").unwrap_or_else(|_| "/data".into());
    let db = db_pool(&data).await?;
    let token_digest = hex::encode(digest(access_token));
    let previous = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key='access_token_digest'",
    )
    .fetch_optional(&db)
    .await?;
    if previous
        .as_deref()
        .is_some_and(|value| value != token_digest)
    {
        sqlx::query("DELETE FROM sessions").execute(&db).await?;
    }
    sqlx::query("INSERT INTO settings(key,value) VALUES('access_token_digest',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        .bind(token_digest).execute(&db).await?;
    let site_id =
        match sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='site_id'")
            .fetch_optional(&db)
            .await?
        {
            Some(id) => id,
            None => {
                let id = Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO settings(key,value) VALUES('site_id',?)")
                    .bind(&id)
                    .execute(&db)
                    .await?;
                id
            }
        };
    // An interrupted send must never be replayed as a new datagram.
    sqlx::query("UPDATE wake_jobs SET status='unknown' WHERE status='sending'")
        .execute(&db)
        .await?;
    load_profiles(&db).await?;
    let public_origin = env::var("WOLUI_PUBLIC_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string());
    let state = AppState {
        db,
        site_id,
        access_digest: digest(access_token),
        secure_cookie: !allow_dev,
        public_origin,
    };
    let web_dir = env::var("WOLUI_WEB_DIR").ok().map(PathBuf::from);
    let app = app(state, web_dir);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr,"WOL UI server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
