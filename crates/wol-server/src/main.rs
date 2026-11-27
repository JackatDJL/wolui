use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::{
    env,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    str::FromStr,
    sync::{mpsc, Arc},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    sync::Semaphore,
    time::{sleep, Duration},
};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;
use wol_core::{Device, MacAddress};

const SESSION_COOKIE: &str = "wolui_session";
const SESSION_SECONDS: i64 = 12 * 60 * 60;
const MAX_PROFILE_NAME_BYTES: usize = 256;
const MAX_CONNECT_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct ApiError {
    code: &'static str,
    message: String,
}

#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    site_id: String,
    site_name: String,
    role: String,
    access_digest: [u8; 32],
    secure_cookie: bool,
    public_origin: Option<String>,
    ssh_gateway_enabled: bool,
    tailscale_bin: String,
    tailscale_socket: String,
    ssh_sessions: Arc<Semaphore>,
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
#[derive(Deserialize, Serialize)]
struct Site {
    id: String,
    name: String,
    local: bool,
    online: bool,
    last_seen_at: Option<i64>,
}
#[derive(Serialize)]
struct Profile {
    id: String,
    name: String,
}
#[derive(Serialize, Deserialize)]
struct FeatureOutput {
    ssh_gateway_enabled: bool,
}
#[derive(Deserialize)]
struct DeviceInput {
    name: String,
    mac: String,
    network_profile_id: String,
    tailscale_name: Option<String>,
    #[serde(default)]
    ssh_enabled: bool,
    #[serde(default)]
    ssh_username: Option<String>,
}
#[derive(Deserialize)]
struct DevicePatch {
    name: Option<String>,
    mac: Option<String>,
    network_profile_id: Option<String>,
    tailscale_name: Option<Option<String>>,
    ssh_enabled: Option<bool>,
    ssh_username: Option<Option<String>>,
    revision: i64,
}
#[derive(Deserialize, Serialize)]
struct WakeOutput {
    id: String,
    status: String,
}
#[derive(Deserialize, Serialize)]
struct LoginOutput {
    csrf_token: String,
    session_token: String,
}
#[derive(Deserialize)]
struct InvitationInput {
    site_name: String,
}
#[derive(Deserialize, Serialize)]
struct InvitationOutput {
    token: String,
    expires_at: i64,
}
#[derive(Deserialize, Serialize)]
struct EnrollInput {
    site_id: String,
    site_name: String,
    invitation: String,
}
#[derive(Deserialize, Serialize)]
struct EnrollOutput {
    credential: String,
}
#[derive(Deserialize, Serialize)]
struct SnapshotProfile {
    id: String,
    name: String,
}
#[derive(Deserialize, Serialize)]
struct SnapshotDevice {
    id: String,
    name: String,
    mac: String,
    network_profile_id: String,
    network_profile_name: String,
    tailscale_name: Option<String>,
    #[serde(default)]
    ssh_enabled: bool,
    #[serde(default)]
    ssh_username: Option<String>,
    revision: i64,
}
#[derive(Deserialize, Serialize)]
struct SnapshotInput {
    site_name: String,
    profiles: Vec<SnapshotProfile>,
    devices: Vec<SnapshotDevice>,
}
#[derive(Serialize, Deserialize)]
struct ConnectCommand {
    id: String,
    device_id: String,
    expires_at: i64,
}
#[derive(Deserialize, Serialize)]
struct CommandResult {
    status: String,
}

async fn current_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        if !token.is_empty() {
            return Some(token.to_owned());
        }
    }
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
fn check_ssh_origin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let accepted = origin.is_some_and(|origin| {
        state
            .public_origin
            .as_deref()
            .is_some_and(|expected| expected.trim_end_matches('/') == origin.trim_end_matches('/'))
    });
    if accepted {
        Ok(())
    } else {
        Err(error(
            StatusCode::FORBIDDEN,
            "origin_forbidden",
            "A same-origin browser request is required",
        ))
    }
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
async fn features(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_session(&state, &headers).await {
        return response;
    }
    Json(FeatureOutput {
        ssh_gateway_enabled: state.ssh_gateway_enabled,
    })
    .into_response()
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
    let mut response = Json(LoginOutput {
        csrf_token: csrf,
        session_token: session,
    })
    .into_response();
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

async fn create_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<InvitationInput>,
) -> Response {
    if state.role != "hub" {
        return error(
            StatusCode::FORBIDDEN,
            "hub_role_required",
            "Set WOLUI_ROLE=hub to create pairing invitations",
        );
    }
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
    let name = input.site_name.trim();
    if name.is_empty() || name.len() > 80 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_site_name",
            "Site name must contain 1 to 80 characters",
        );
    }
    let token = format!("wolc_{}", random_secret());
    let expires_at = now() + 600;
    match sqlx::query("INSERT INTO connect_invitations(token_digest,site_name,expires_at,created_at) VALUES(?,?,?,?)")
        .bind(hex::encode(digest(&token))).bind(name).bind(expires_at).bind(now()).execute(&state.db).await {
        Ok(_) => (StatusCode::CREATED,Json(InvitationOutput{token,expires_at})).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not create invitation"),
    }
}

async fn enroll(State(state): State<AppState>, Json(input): Json<EnrollInput>) -> Response {
    if input.site_id.trim().is_empty()
        || input.site_id.len() > 80
        || input.site_name.trim().is_empty()
        || input.site_name.len() > 80
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_site",
            "Site identity is invalid",
        );
    }
    let credential = format!("wols_{}", random_secret());
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not begin pairing",
            )
        }
    };
    let invite = sqlx::query_as::<_,(String,i64)>("SELECT site_name,expires_at FROM connect_invitations WHERE token_digest=? AND used_at IS NULL")
        .bind(hex::encode(digest(&input.invitation))).fetch_optional(&mut *tx).await;
    let Some((invited_name, expires_at)) = (match invite {
        Ok(v) => v,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not check invitation",
            )
        }
    }) else {
        return error(
            StatusCode::UNAUTHORIZED,
            "invitation_invalid",
            "Invitation is invalid or has already been used",
        );
    };
    if expires_at <= now() {
        return error(
            StatusCode::UNAUTHORIZED,
            "invitation_expired",
            "Invitation has expired",
        );
    }
    let claimed = sqlx::query("UPDATE connect_invitations SET used_at=? WHERE token_digest=? AND used_at IS NULL AND expires_at>? ")
        .bind(now()).bind(hex::encode(digest(&input.invitation))).bind(now()).execute(&mut *tx).await;
    if !matches!(claimed, Ok(ref result) if result.rows_affected() == 1) {
        return error(
            StatusCode::CONFLICT,
            "invitation_claimed",
            "Invitation has already been claimed",
        );
    }
    let inserted = sqlx::query("INSERT INTO connect_sites(id,name,token_digest,is_local,active,last_seen_at,created_at) VALUES(?,?,?,0,1,?,?)")
        .bind(&input.site_id).bind(invited_name.trim()).bind(hex::encode(digest(&credential))).bind(now()).bind(now()).execute(&mut *tx).await;
    if inserted.is_err() {
        return error(
            StatusCode::CONFLICT,
            "site_exists",
            "This site is already paired",
        );
    }
    if tx.commit().await.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not finish pairing",
        );
    }
    tracing::info!(site_id=%input.site_id, invited_name=%invited_name, "WOL Connect site paired");
    Json(EnrollOutput { credential }).into_response()
}

async fn connect_site_id(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), Response> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| {
            error(
                StatusCode::UNAUTHORIZED,
                "connect_unauthorized",
                "A Connect credential is required",
            )
        })?;
    let token_digest = hex::encode(digest(bearer));
    let site_id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM connect_sites WHERE token_digest=? AND active=1 AND is_local=0",
    )
    .bind(&token_digest)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not validate Connect credential",
        )
    })?;
    site_id
        .map(|site_id| (site_id, token_digest))
        .ok_or_else(|| {
            error(
                StatusCode::UNAUTHORIZED,
                "connect_unauthorized",
                "Connect credential is invalid or revoked",
            )
        })
}

async fn connect_credential_is_current(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    site_id: &str,
    token_digest: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM connect_sites WHERE id=? AND token_digest=? AND active=1 AND is_local=0)",
    )
    .bind(site_id)
    .bind(token_digest)
    .fetch_one(&mut **tx)
    .await
    .map(|exists| exists == 1)
}

async fn connect_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SnapshotInput>,
) -> Response {
    let (site_id, token_digest) = match connect_site_id(&state, &headers).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    if input.devices.len() > 10_000
        || input.profiles.len() > 256
        || input.site_name.trim().is_empty()
        || input.site_name.len() > 80
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "snapshot_invalid",
            "Inventory snapshot exceeds its limits or contains an invalid site name",
        );
    }
    let profile_ids: std::collections::HashSet<_> =
        input.profiles.iter().map(|p| p.id.as_str()).collect();
    for profile in &input.profiles {
        if profile.id.is_empty()
            || profile.id.len() > 80
            || profile.name.trim().is_empty()
            || profile.name.len() > MAX_PROFILE_NAME_BYTES
        {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "snapshot_invalid",
                "Network profile is invalid",
            );
        }
    }
    let mut device_ids = std::collections::HashSet::new();
    for device in &input.devices {
        if device.id.is_empty()
            || device.id.len() > 80
            || device.name.trim().is_empty()
            || device.name.len() > 120
            || device.mac.parse::<MacAddress>().is_err()
            || device
                .tailscale_name
                .as_deref()
                .is_some_and(|host| !valid_ssh_host(host))
            || device
                .ssh_username
                .as_deref()
                .is_some_and(|user| !valid_ssh_username(user))
            || (device.ssh_enabled
                && (device.tailscale_name.is_none() || device.ssh_username.is_none()))
            || !profile_ids.contains(device.network_profile_id.as_str())
        {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "snapshot_invalid",
                "Device inventory is invalid",
            );
        }
        if !device_ids.insert(device.id.as_str()) {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "snapshot_invalid",
                "Device ids must be unique",
            );
        }
    }
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not update site inventory",
            )
        }
    };
    match connect_credential_is_current(&mut tx, &site_id, &token_digest).await {
        Ok(true) => {}
        Ok(false) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "connect_unauthorized",
                "Connect credential is invalid or revoked",
            )
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not validate Connect credential",
            )
        }
    }
    if sqlx::query("DELETE FROM remote_devices WHERE site_id=?")
        .bind(&site_id)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not replace site inventory",
        );
    }
    for device in &input.devices {
        let profile_name = input
            .profiles
            .iter()
            .find(|p| p.id == device.network_profile_id)
            .map(|p| p.name.as_str())
            .unwrap_or("");
        if sqlx::query("INSERT INTO remote_devices(site_id,id,name,mac,network_profile_id,network_profile_name,tailscale_name,revision,ssh_enabled,ssh_username) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&site_id).bind(&device.id).bind(device.name.trim()).bind(device.mac.to_ascii_lowercase()).bind(&device.network_profile_id).bind(profile_name).bind(&device.tailscale_name).bind(device.revision).bind(device.ssh_enabled).bind(&device.ssh_username).execute(&mut *tx).await.is_err() {
            return error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not write site inventory");
        }
    }
    let heartbeat = sqlx::query("UPDATE connect_sites SET last_seen_at=? WHERE id=? AND active=1 AND is_local=0 AND token_digest=?")
        .bind(now())
        .bind(&site_id)
        .bind(&token_digest)
        .execute(&mut *tx)
        .await;
    if !matches!(heartbeat, Ok(ref result) if result.rows_affected() == 1) {
        return error(
            StatusCode::UNAUTHORIZED,
            "connect_unauthorized",
            "Connect credential is invalid or revoked",
        );
    }
    if tx.commit().await.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not commit site inventory",
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn next_connect_command(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (site_id, token_digest) = match connect_site_id(&state, &headers).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not read command queue",
            )
        }
    };
    match connect_credential_is_current(&mut tx, &site_id, &token_digest).await {
        Ok(true) => {}
        Ok(false) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "connect_unauthorized",
                "Connect credential is invalid or revoked",
            )
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not validate Connect credential",
            )
        }
    }
    if sqlx::query("UPDATE connect_commands SET status='unknown' WHERE site_id=? AND status IN ('queued','accepted') AND expires_at<=? AND EXISTS(SELECT 1 FROM connect_sites WHERE id=? AND token_digest=? AND active=1 AND is_local=0)")
        .bind(&site_id).bind(now()).bind(&site_id).bind(&token_digest).execute(&mut *tx).await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not expire queued commands");
    }
    let row = sqlx::query_as::<_,(String,String,i64)>("SELECT id,device_id,expires_at FROM connect_commands WHERE site_id=? AND status='queued' AND expires_at>? ORDER BY created_at LIMIT 1")
        .bind(&site_id).bind(now()).fetch_optional(&mut *tx).await;
    let Some((id, device_id, expires_at)) = (match row {
        Ok(v) => v,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not read command queue",
            )
        }
    }) else {
        if tx.commit().await.is_err() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not commit command poll",
            );
        }
        return StatusCode::NO_CONTENT.into_response();
    };
    let reservation = sqlx::query("UPDATE connect_commands SET status='accepted' WHERE id=? AND status='queued' AND EXISTS(SELECT 1 FROM connect_sites WHERE id=? AND token_digest=? AND active=1 AND is_local=0)")
        .bind(&id)
        .bind(&site_id)
        .bind(&token_digest)
        .execute(&mut *tx)
        .await;
    if !matches!(reservation, Ok(ref result) if result.rows_affected() == 1) {
        return error(
            StatusCode::CONFLICT,
            "command_unavailable",
            "Wake command could not be reserved",
        );
    }
    if tx.commit().await.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not reserve wake command",
        );
    }
    Json(ConnectCommand {
        id,
        device_id,
        expires_at,
    })
    .into_response()
}

async fn connect_command_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<CommandResult>,
) -> Response {
    let (site_id, token_digest) = match connect_site_id(&state, &headers).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !["sent", "failed", "unknown"].contains(&input.status.as_str()) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_status",
            "Command result status is invalid",
        );
    }
    match sqlx::query("UPDATE connect_commands SET status=? WHERE id=? AND site_id=? AND status='accepted' AND expires_at>? AND EXISTS(SELECT 1 FROM connect_sites WHERE id=? AND token_digest=? AND active=1 AND is_local=0)")
        .bind(&input.status).bind(id).bind(&site_id).bind(now()).bind(&site_id).bind(&token_digest).execute(&state.db).await {
        Ok(result) if result.rows_affected()==1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => error(StatusCode::CONFLICT,"command_expired","Command is expired, unknown, or already completed"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not record wake result"),
    }
}

async fn connect_sites(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    match sqlx::query_as::<_, (String, String, Option<i64>)>(
        "SELECT id,name,last_seen_at FROM connect_sites WHERE is_local=0 ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|(id, name, last_seen_at)| Site {
                    id,
                    name,
                    local: false,
                    online: last_seen_at.is_some_and(|at| now() - at <= 15),
                    last_seen_at,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not load paired sites",
        ),
    }
}

async fn revoke_connect_site(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(site): Path<String>,
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
    match sqlx::query("DELETE FROM connect_sites WHERE id=? AND is_local=0")
        .bind(site)
        .execute(&state.db)
        .await
    {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => error(
            StatusCode::NOT_FOUND,
            "site_not_found",
            "Paired site not found",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Could not revoke paired site",
        ),
    }
}

async fn sites(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    let mut result = vec![Site {
        id: state.site_id.clone(),
        name: state.site_name.clone(),
        local: true,
        online: true,
        last_seen_at: Some(now()),
    }];
    match sqlx::query_as::<_, (String, String, Option<i64>)>("SELECT id,name,last_seen_at FROM connect_sites WHERE is_local=0 AND active=1 ORDER BY name")
        .fetch_all(&state.db).await {
        Ok(rows) => result.extend(rows.into_iter().map(|(id,name,last_seen_at)| Site { id, name, local: false, online: last_seen_at.is_some_and(|v| now() - v <= 15), last_seen_at })),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not load sites"),
    }
    Json(result).into_response()
}
async fn profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(site): Path<String>,
) -> Response {
    if let Err(r) = require_session(&state, &headers).await {
        return r;
    }
    if site == state.site_id {
        match sqlx::query_as::<_, (String, String)>(
            "SELECT id,name FROM network_profiles ORDER BY name",
        )
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => Json(
                rows.into_iter()
                    .map(|(id, name)| Profile { id, name })
                    .collect::<Vec<_>>(),
            )
            .into_response(),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load network profiles",
            ),
        }
    } else {
        match sqlx::query_as::<_, (String,String)>("SELECT DISTINCT network_profile_id,network_profile_name FROM remote_devices WHERE site_id=? ORDER BY network_profile_name").bind(site).fetch_all(&state.db).await {
        Ok(rows) => Json(rows.into_iter().map(|(id,name)| Profile { id,name }).collect::<Vec<_>>()).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not load remote network profiles"),
      }
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
    if site == state.site_id {
        match fetch_devices(&state.db, None).await {
            Ok(items) => Json(items).into_response(),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load devices",
            ),
        }
    } else {
        match sqlx::query_as::<_, (String,String,String,String,Option<String>,bool,Option<String>,i64)>("SELECT id,name,mac,network_profile_id,tailscale_name,ssh_enabled,ssh_username,revision FROM remote_devices WHERE site_id=? ORDER BY name").bind(site).fetch_all(&state.db).await {
            Ok(items) => Json(items.into_iter().map(|(id,name,mac,network_profile_id,tailscale_name,ssh_enabled,ssh_username,revision)| Device{id,name,mac,network_profile_id,tailscale_name,ssh_enabled,ssh_username,revision}).collect::<Vec<_>>()).into_response(),
            Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error","Could not load remote devices"),
        }
    }
}
async fn fetch_devices(db: &SqlitePool, id: Option<&str>) -> Result<Vec<Device>, sqlx::Error> {
    let rows = if let Some(id) = id {
        sqlx::query_as::<_, (String, String, String, String, Option<String>, bool, Option<String>, i64)>(
            "SELECT id,name,mac,network_profile_id,tailscale_name,ssh_enabled,ssh_username,revision FROM devices WHERE id=?",
        )
        .bind(id)
        .fetch_all(db)
        .await?
    } else {
        sqlx::query_as::<_,(String,String,String,String,Option<String>,bool,Option<String>,i64)>("SELECT id,name,mac,network_profile_id,tailscale_name,ssh_enabled,ssh_username,revision FROM devices ORDER BY name").fetch_all(db).await?
    };
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                mac,
                network_profile_id,
                tailscale_name,
                ssh_enabled,
                ssh_username,
                revision,
            )| Device {
                id,
                name,
                mac,
                network_profile_id,
                tailscale_name,
                ssh_enabled,
                ssh_username,
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
    if input
        .tailscale_name
        .as_deref()
        .is_some_and(|host| !valid_ssh_host(host))
    {
        return Err((
            "invalid_tailscale_name",
            "Tailscale host must be a valid MagicDNS name or Tailscale IP",
        ));
    }
    if input
        .ssh_username
        .as_deref()
        .is_some_and(|user| !valid_ssh_username(user))
    {
        return Err((
            "invalid_ssh_username",
            "SSH username may contain only letters, digits, dot, underscore, and hyphen",
        ));
    }
    if input.ssh_enabled && (input.tailscale_name.is_none() || input.ssh_username.is_none()) {
        return Err((
            "ssh_target_required",
            "SSH requires a Tailscale host and username",
        ));
    }
    Ok(())
}

fn valid_ssh_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn valid_ssh_host(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.starts_with('-') {
        return false;
    }
    if value.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
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
    let result = sqlx::query("INSERT INTO devices(id,name,mac,network_profile_id,tailscale_name,revision,ssh_enabled,ssh_username) SELECT ?,?,?,?, ?,1,?,? WHERE EXISTS(SELECT 1 FROM network_profiles WHERE id=?)")
        .bind(&id).bind(input.name.trim()).bind(input.mac.to_ascii_lowercase()).bind(&input.network_profile_id).bind(&input.tailscale_name).bind(input.ssh_enabled).bind(&input.ssh_username).bind(&input.network_profile_id).execute(&state.db).await;
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
        ssh_enabled: input.ssh_enabled.unwrap_or(old.ssh_enabled),
        ssh_username: input.ssh_username.unwrap_or(old.ssh_username),
    };
    if let Err((code, msg)) = validate_input(&candidate) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, code, msg);
    }
    let result=sqlx::query("UPDATE devices SET name=?,mac=?,network_profile_id=?,tailscale_name=?,ssh_enabled=?,ssh_username=?,revision=revision+1 WHERE id=? AND revision=? AND EXISTS(SELECT 1 FROM network_profiles WHERE id=?)")
        .bind(candidate.name.trim()).bind(candidate.mac.to_ascii_lowercase()).bind(&candidate.network_profile_id).bind(&candidate.tailscale_name).bind(candidate.ssh_enabled).bind(&candidate.ssh_username).bind(&id).bind(input.revision).bind(&candidate.network_profile_id).execute(&state.db).await;
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
    if site != state.site_id {
        if let Some((job, status, device_id)) = match sqlx::query_as::<_, (String, String, String)>(
            "SELECT id,status,device_id FROM connect_commands WHERE site_id=? AND idempotency_key=?",
        )
        .bind(&site)
        .bind(key)
        .fetch_optional(&state.db)
        .await
        {
            Ok(existing) => existing,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Could not check remote wake idempotency",
                )
            }
        } {
            if device_id != id {
                return error(StatusCode::CONFLICT,"idempotency_conflict","This Idempotency-Key was used for a different operation");
            }
            return (StatusCode::ACCEPTED,Json(WakeOutput{id:job,status})).into_response();
        }
        let remote = sqlx::query_as::<_, (Option<i64>,)>(
            "SELECT last_seen_at FROM connect_sites WHERE id=? AND is_local=0 AND active=1",
        )
        .bind(&site)
        .fetch_optional(&state.db)
        .await;
        let Some((Some(last_seen),)) = (match remote {
            Ok(v) => v,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Could not check remote site",
                )
            }
        }) else {
            return error(StatusCode::NOT_FOUND, "site_not_found", "Site not found");
        };
        if now() - last_seen > 15 {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "site_offline",
                "Site is offline; wake was not queued",
            );
        }
        let device_exists = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM remote_devices WHERE site_id=? AND id=?)",
        )
        .bind(&site)
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
            == 1;
        if !device_exists {
            return error(
                StatusCode::NOT_FOUND,
                "device_not_found",
                "Device not found",
            );
        }
        let job = Uuid::new_v4().to_string();
        match sqlx::query("INSERT INTO connect_commands(id,site_id,device_id,idempotency_key,status,expires_at,created_at) VALUES(?,?,?,?,'queued',?,?)")
            .bind(&job).bind(&site).bind(&id).bind(key).bind(now()+30).bind(now()).execute(&state.db).await {
            Ok(_) => return (StatusCode::ACCEPTED,Json(WakeOutput{id:job,status:"queued".into()})).into_response(),
            Err(_) => if let Ok(Some((existing,status,device_id))) = sqlx::query_as::<_,(String,String,String)>("SELECT id,status,device_id FROM connect_commands WHERE site_id=? AND idempotency_key=?").bind(&site).bind(key).fetch_optional(&state.db).await {
                if device_id != id { return error(StatusCode::CONFLICT,"idempotency_conflict","This Idempotency-Key was used for a different operation"); }
                return (StatusCode::ACCEPTED,Json(WakeOutput{id:existing,status})).into_response();
            } else { return error(StatusCode::CONFLICT,"idempotency_conflict","Could not reserve remote wake request"); }
        }
    }
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
        let _ = sqlx::query("UPDATE connect_commands SET status='unknown' WHERE id=? AND site_id=? AND status IN ('queued','accepted') AND expires_at<=?").bind(&id).bind(&site).bind(now()).execute(&state.db).await;
        return match sqlx::query_as::<_, (String, String)>(
            "SELECT id,status FROM connect_commands WHERE id=? AND site_id=?",
        )
        .bind(&id)
        .bind(&site)
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
                "Could not load remote wake job",
            ),
        };
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

#[derive(Deserialize)]
struct TerminalResize {
    #[serde(rename = "type")]
    kind: String,
    cols: u16,
    rows: u16,
}

async fn ssh_terminal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((site, device)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(response) = check_ssh_origin(&state, &headers) {
        return response;
    }
    if !state.ssh_gateway_enabled {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ssh_gateway_disabled",
            "The Tailscale SSH gateway is disabled on this server",
        );
    }
    let session_token = match require_session(&state, &headers).await {
        Ok((token, _)) => token,
        Err(response) => return response,
    };
    let permit = match state.ssh_sessions.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "ssh_session_limit",
                "This server has reached its active SSH session limit",
            )
        }
    };
    let target = if site == state.site_id {
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT tailscale_name,ssh_username FROM devices WHERE id=? AND ssh_enabled=1",
        )
        .bind(&device)
        .fetch_optional(&state.db)
        .await
    } else {
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT d.tailscale_name,d.ssh_username FROM remote_devices d JOIN connect_sites s ON s.id=d.site_id WHERE d.site_id=? AND d.id=? AND d.ssh_enabled=1 AND s.active=1",
        )
        .bind(&site)
        .bind(&device)
        .fetch_optional(&state.db)
        .await
    };
    let (host, username) = match target {
        Ok(Some((Some(host), Some(username)))) => (host, username),
        Ok(_) => {
            return error(
                StatusCode::NOT_FOUND,
                "ssh_not_enabled",
                "SSH is not enabled for this device",
            )
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Could not load SSH target",
            )
        }
    };
    if !valid_ssh_host(&host) || !valid_ssh_username(&username) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ssh_target_invalid",
            "The stored Tailscale SSH target is invalid",
        );
    }
    let session_id = Uuid::new_v4().to_string();
    let session_digest = hex::encode(digest(&session_token));
    ws.max_message_size(32 * 1024)
        .max_frame_size(32 * 1024)
        .on_upgrade(move |socket| {
            run_ssh_terminal(
                socket,
                state,
                permit,
                session_id,
                session_digest,
                site,
                device,
                username,
                host,
            )
        })
}

async fn run_ssh_terminal(
    mut socket: WebSocket,
    state: AppState,
    _permit: tokio::sync::OwnedSemaphorePermit,
    session_id: String,
    session_digest: String,
    site_id: String,
    device_id: String,
    username: String,
    host: String,
) {
    let started_at = now();
    if sqlx::query("INSERT INTO ssh_sessions(id,site_id,device_id,started_at,outcome) VALUES(?,?,?,?, 'opening')")
        .bind(&session_id).bind(&site_id).bind(&device_id).bind(started_at)
        .execute(&state.db).await.is_err() {
        let _ = socket.send(Message::Text(serde_json::json!({"type":"error","message":"Could not record SSH session metadata"}).to_string())).await;
        return;
    }

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!(%err, "could not create SSH terminal PTY");
            let _ = socket.send(Message::Text(serde_json::json!({"type":"error","message":"The terminal could not be opened on this server"}).to_string())).await;
            finish_ssh_session(&state, &session_id, "start_failed").await;
            return;
        }
    };
    let mut command = CommandBuilder::new(&state.tailscale_bin);
    command.arg(format!("--socket={}", state.tailscale_socket));
    command.arg("ssh");
    command.arg(format!("{username}@{host}"));
    command.env("TERM", "xterm-256color");
    let mut child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(err) => {
            tracing::warn!(%err, "could not start Tailscale SSH client");
            let _ = socket.send(Message::Text(serde_json::json!({"type":"error","message":"Tailscale SSH could not start. Check the CLI and local Tailscale connection."}).to_string())).await;
            finish_ssh_session(&state, &session_id, "start_failed").await;
            return;
        }
    };
    drop(pair.slave);
    let mut reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(err) => {
            tracing::warn!(%err, "could not read SSH terminal PTY");
            let _ = child.kill();
            let _ = child.wait();
            finish_ssh_session(&state, &session_id, "start_failed").await;
            return;
        }
    };
    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(err) => {
            tracing::warn!(%err, "could not write SSH terminal PTY");
            let _ = child.kill();
            let _ = child.wait();
            finish_ssh_session(&state, &session_id, "start_failed").await;
            return;
        }
    };
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    if output_tx.blocking_send(buffer[..size].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut writer = writer;
        while let Ok(bytes) = input_rx.recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
        }
    });

    if socket
        .send(Message::Text(
            serde_json::json!({"type":"ready","session_id":session_id.clone()}).to_string(),
        ))
        .await
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        finish_ssh_session(&state, &session_id, "disconnected").await;
        return;
    }
    let mut idle_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(15 * 60)));
    let mut authorization_check = tokio::time::interval(Duration::from_secs(30));
    authorization_check.tick().await;
    let outcome = loop {
        tokio::select! {
            output = output_rx.recv() => match output {
                Some(bytes) => {
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15 * 60));
                    if socket.send(Message::Binary(bytes.into())).await.is_err() { break "disconnected"; }
                },
                None => break "closed",
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15 * 60));
                    if input_tx.send(bytes.to_vec()).is_err() { break "closed"; }
                },
                Some(Ok(Message::Text(text))) => {
                    if let Ok(resize) = serde_json::from_str::<TerminalResize>(&text) {
                        if resize.kind == "resize" {
                            let size = PtySize {
                                rows: resize.rows.clamp(5, 120),
                                cols: resize.cols.clamp(20, 300),
                                pixel_width: 0,
                                pixel_height: 0,
                            };
                            let _ = pair.master.resize(size);
                        }
                    }
                },
                Some(Ok(Message::Ping(payload))) => { if socket.send(Message::Pong(payload)).await.is_err() { break "disconnected"; } },
                Some(Ok(Message::Pong(_))) => {},
                Some(Ok(Message::Close(_))) | None => break "disconnected",
                Some(Err(_)) => break "disconnected",
            },
            _ = authorization_check.tick() => {
                match ssh_authorization_is_current(&state, &session_digest, &site_id, &device_id, &username, &host).await {
                    Ok(true) => {},
                    Ok(false) => {
                        let _ = socket.send(Message::Text(serde_json::json!({"type":"error","message":"SSH access or the web session was revoked"}).to_string())).await;
                        break "revoked";
                    },
                    Err(err) => {
                        tracing::warn!(%err, "SSH authorization recheck failed");
                        break "authorization_check_failed";
                    },
                }
            }
            _ = &mut idle_deadline => {
                let _ = socket.send(Message::Text(serde_json::json!({"type":"error","message":"SSH session closed after 15 minutes without terminal activity"}).to_string())).await;
                break "idle_timeout";
            }
        }
    };
    drop(input_tx);
    let _ = child.kill();
    let _ = tokio::task::spawn_blocking(move || child.wait()).await;
    finish_ssh_session(&state, &session_id, outcome).await;
}

async fn ssh_authorization_is_current(
    state: &AppState,
    session_digest: &str,
    site_id: &str,
    device_id: &str,
    username: &str,
    host: &str,
) -> Result<bool, sqlx::Error> {
    let session_valid = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE token_digest=? AND expires_at>?)",
    )
    .bind(session_digest)
    .bind(now())
    .fetch_one(&state.db)
    .await?
        == 1;
    if !session_valid {
        return Ok(false);
    }
    let target_valid = if site_id == state.site_id {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM devices WHERE id=? AND ssh_enabled=1 AND tailscale_name=? AND ssh_username=?)",
        )
        .bind(device_id)
        .bind(host)
        .bind(username)
        .fetch_one(&state.db)
        .await?
            == 1
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM remote_devices d JOIN connect_sites s ON s.id=d.site_id WHERE d.site_id=? AND d.id=? AND d.ssh_enabled=1 AND d.tailscale_name=? AND d.ssh_username=? AND s.active=1)",
        )
        .bind(site_id)
        .bind(device_id)
        .bind(host)
        .bind(username)
        .fetch_one(&state.db)
        .await?
            == 1
    };
    Ok(target_valid)
}

async fn finish_ssh_session(state: &AppState, session_id: &str, outcome: &str) {
    let _ = sqlx::query("UPDATE ssh_sessions SET ended_at=?,outcome=? WHERE id=?")
        .bind(now())
        .bind(outcome)
        .bind(session_id)
        .execute(&state.db)
        .await;
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

async fn local_snapshot(state: &AppState) -> Result<SnapshotInput, sqlx::Error> {
    let profiles =
        sqlx::query_as::<_, (String, String)>("SELECT id,name FROM network_profiles ORDER BY name")
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .map(|(id, name)| SnapshotProfile { id, name })
            .collect();
    let devices = sqlx::query_as::<_,(String,String,String,String,String,Option<String>,bool,Option<String>,i64)>("SELECT d.id,d.name,d.mac,d.network_profile_id,p.name,d.tailscale_name,d.ssh_enabled,d.ssh_username,d.revision FROM devices d JOIN network_profiles p ON p.id=d.network_profile_id ORDER BY d.name").fetch_all(&state.db).await?
        .into_iter().map(|(id,name,mac,network_profile_id,network_profile_name,tailscale_name,ssh_enabled,ssh_username,revision)|SnapshotDevice{id,name,mac,network_profile_id,network_profile_name,tailscale_name,ssh_enabled,ssh_username,revision}).collect();
    Ok(SnapshotInput {
        site_name: state.site_name.clone(),
        profiles,
        devices,
    })
}

async fn dispatch_local_wake(
    state: &AppState,
    device_id: &str,
    key: &str,
    deadline: Option<i64>,
) -> Result<WakeOutput, &'static str> {
    if let Some((id, status, existing_device)) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id,status,device_id FROM wake_jobs WHERE idempotency_key=?",
    )
    .bind(key)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| "Could not check wake idempotency")?
    {
        if existing_device != device_id {
            return Err("Idempotency key belongs to another device");
        }
        return Ok(WakeOutput { id, status });
    }
    let row = sqlx::query_as::<_,(String,String,String,u16)>("SELECT d.mac,p.bind_address,p.broadcast_address,p.port FROM devices d JOIN network_profiles p ON p.id=d.network_profile_id WHERE d.id=?")
        .bind(device_id).fetch_optional(&state.db).await.map_err(|_|"Could not load wake target")?;
    let Some((mac, bind, broadcast, port)) = row else {
        return Err("Device not found");
    };
    let job = Uuid::new_v4().to_string();
    if sqlx::query("INSERT INTO wake_jobs(id,idempotency_key,device_id,status,created_at) VALUES(?,?,?,'sending',?)")
        .bind(&job).bind(key).bind(device_id).bind(now()).execute(&state.db).await.is_err() {
        if let Some((id,status,existing_device))=sqlx::query_as::<_,(String,String,String)>("SELECT id,status,device_id FROM wake_jobs WHERE idempotency_key=?").bind(key).fetch_optional(&state.db).await.map_err(|_|"Could not reserve wake request")? {
            if existing_device != device_id { return Err("Idempotency key belongs to another device"); }
            return Ok(WakeOutput{id,status});
        }
        return Err("Could not reserve wake request");
    }
    let status = if deadline.is_some_and(|deadline| deadline <= now()) {
        "unknown"
    } else {
        match (
            mac.parse::<MacAddress>(),
            bind.parse::<Ipv4Addr>(),
            broadcast.parse::<Ipv4Addr>(),
        ) {
            (Ok(mac), Ok(bind), Ok(broadcast)) => send_magic_packet(mac, bind, broadcast, port)
                .map(|_| "sent")
                .unwrap_or("failed"),
            _ => "failed",
        }
    };
    if let Err(err) = sqlx::query("UPDATE wake_jobs SET status=? WHERE id=?")
        .bind(status)
        .bind(&job)
        .execute(&state.db)
        .await
    {
        tracing::error!(%err, wake_job_id=%job, observed_status=status, "Magic packet result could not be persisted");
    }
    Ok(WakeOutput {
        id: job,
        status: status.into(),
    })
}

fn connector_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| "WOLUI_CONNECT_URL must be an absolute URL".to_owned())?;
    let secure = url.scheme() == "https";
    let local_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if !secure && !local_http {
        return Err("WOLUI_CONNECT_URL must use HTTPS (HTTP is allowed only for localhost)".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("WOLUI_CONNECT_URL must not contain query or fragment values".into());
    }
    Ok(url)
}

async fn persist_connector_credential(
    path: &PathBuf,
    credential: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !valid_connect_credential(credential) {
        return Err("refusing to persist an invalid WOL Connect credential".into());
    }
    let target = path.clone();
    let credential = credential.to_owned();
    tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
        use std::io::Write;

        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        let name = target
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("connect-token"))
            .to_string_lossy();
        let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(credential.as_bytes())?;
            file.sync_all()?;
            // hard_link publishes atomically and refuses to replace an existing credential.
            std::fs::hard_link(&temporary, &target)?;
            std::fs::remove_file(&temporary)?;
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    })
    .await??;
    Ok(())
}

fn valid_connect_credential(credential: &str) -> bool {
    credential.strip_prefix("wols_").is_some_and(|secret| {
        secret.len() == 48 && secret.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

async fn run_connector(
    state: AppState,
    hub_url: String,
    invitation_path: PathBuf,
    credential_path: PathBuf,
) {
    let base = match connector_url(&hub_url) {
        Ok(url) => url,
        Err(err) => {
            tracing::error!(%err,"WOL Connect disabled due to invalid URL");
            return;
        }
    };
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .user_agent(concat!("wolui-connect/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::error!(%err,"Could not initialize WOL Connect client");
            return;
        }
    };
    let mut credential = match tokio::fs::read_to_string(&credential_path).await {
        Ok(value) if valid_connect_credential(value.trim()) => Some(value.trim().to_owned()),
        Ok(_) => {
            tracing::error!(path=%credential_path.display(),"Stored WOL Connect credential is empty or malformed; remove it and re-pair this site");
            return;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            tracing::error!(%err,"Could not read WOL Connect credential");
            return;
        }
    };
    if credential.is_none() {
        let invitation = match tokio::fs::read_to_string(&invitation_path).await {
            Ok(value) => value.trim().to_owned(),
            Err(err) => {
                tracing::error!(%err, "Could not read WOL Connect invitation file");
                return;
            }
        };
        let endpoint = base.join("api/v1/connect/enroll").expect("relative URL");
        let input = EnrollInput {
            site_id: state.site_id.clone(),
            site_name: state.site_name.clone(),
            invitation,
        };
        let response = match client.post(endpoint).json(&input).send().await {
            Ok(r) => r,
            Err(err) => {
                tracing::error!(%err,"WOL Connect pairing request failed");
                return;
            }
        };
        if !response.status().is_success() {
            tracing::error!(status=%response.status(),"WOL Connect pairing was rejected");
            return;
        }
        let enrolled = match response.json::<EnrollOutput>().await {
            Ok(value) => value,
            Err(err) => {
                tracing::error!(%err,"WOL Connect returned an invalid pairing response");
                return;
            }
        };
        if let Err(err) = persist_connector_credential(&credential_path, &enrolled.credential).await
        {
            tracing::error!(%err,"Could not persist WOL Connect credential");
            return;
        }
        if let Err(err) = tokio::fs::remove_file(&invitation_path).await {
            tracing::warn!(%err,"Could not remove consumed WOL Connect invitation file");
        }
        credential = Some(enrolled.credential);
        tracing::info!(hub=%base,"WOL Connect pairing completed");
    }
    let credential = credential.unwrap();
    loop {
        let snapshot = match local_snapshot(&state).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(%err,"Could not build WOL Connect inventory");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let snapshot_url = base.join("api/v1/connect/snapshot").expect("relative URL");
        let response = client
            .post(snapshot_url)
            .bearer_auth(&credential)
            .json(&snapshot)
            .send()
            .await;
        match response {
            Ok(r) if r.status() == StatusCode::UNAUTHORIZED => {
                tracing::error!("WOL Connect credential was revoked; stopping connector");
                return;
            }
            Ok(r) if !r.status().is_success() => {
                tracing::warn!(status=%r.status(),"WOL Connect inventory update failed")
            }
            Err(err) => tracing::warn!(%err,"WOL Connect inventory update failed"),
            _ => {}
        }
        let command_url = base
            .join("api/v1/connect/commands/next")
            .expect("relative URL");
        match client
            .get(command_url)
            .bearer_auth(&credential)
            .send()
            .await
        {
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                tracing::error!("WOL Connect credential was revoked; stopping connector");
                return;
            }
            Ok(response) if response.status() == StatusCode::NO_CONTENT => {}
            Ok(response) if response.status().is_success() => {
                match response.json::<ConnectCommand>().await {
                    Ok(command) => {
                        let status = if command.expires_at <= now() {
                            "unknown".to_owned()
                        } else {
                            dispatch_local_wake(
                                &state,
                                &command.device_id,
                                &format!("connect:{}", command.id),
                                Some(command.expires_at),
                            )
                            .await
                            .map(|job| job.status)
                            .unwrap_or_else(|_| "failed".into())
                        };
                        let result_url = base
                            .join(&format!("api/v1/connect/commands/{}/result", command.id))
                            .expect("relative URL");
                        if let Err(err) = client
                            .post(result_url)
                            .bearer_auth(&credential)
                            .json(&CommandResult { status })
                            .send()
                            .await
                        {
                            tracing::warn!(%err,"Could not report WOL Connect result");
                        }
                    }
                    Err(err) => tracing::warn!(%err,"WOL Connect returned an invalid command"),
                }
            }
            Ok(response) => {
                tracing::warn!(status=%response.status(),"WOL Connect command poll failed")
            }
            Err(err) => tracing::warn!(%err,"WOL Connect command poll failed"),
        }
        sleep(Duration::from_secs(5)).await;
    }
}

fn app(state: AppState, web_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/sessions", post(login))
        .route("/api/v1/sessions/current", get(current).delete(logout))
        .route("/api/v1/features", get(features))
        .route("/api/v1/connect/invitations", post(create_invitation))
        .route("/api/v1/connect/sites", get(connect_sites))
        .route(
            "/api/v1/connect/sites/:site",
            axum::routing::delete(revoke_connect_site),
        )
        .route("/api/v1/connect/enroll", post(enroll))
        .route(
            "/api/v1/connect/snapshot",
            post(connect_snapshot).layer(DefaultBodyLimit::max(MAX_CONNECT_SNAPSHOT_BYTES)),
        )
        .route("/api/v1/connect/commands/next", get(next_connect_command))
        .route(
            "/api/v1/connect/commands/:id/result",
            post(connect_command_result),
        )
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
        .route("/api/v1/sites/:site/devices/:id/ssh", get(ssh_terminal))
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
                site_name: "Test site".into(),
                role: "hub".into(),
                access_digest: digest("0123456789012345678901234567890123456789"),
                secure_cookie: false,
                public_origin: Some("http://localhost".into()),
                ssh_gateway_enabled: false,
                tailscale_bin: "tailscale".into(),
                tailscale_socket: "/var/run/tailscale/tailscaled.sock".into(),
                ssh_sessions: Arc::new(Semaphore::new(4)),
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
        let features_response = router
            .clone()
            .oneshot(
                Request::get("/api/v1/features")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(features_response.status(), StatusCode::OK);
        let features: FeatureOutput = serde_json::from_slice(
            &to_bytes(features_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(!features.ssh_gateway_enabled);
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

    #[tokio::test]
    async fn ssh_origin_check_rejects_missing_and_foreign_origins() {
        let (state, _receiver) = test_state().await;
        assert!(check_ssh_origin(&state, &HeaderMap::new()).is_err());
        let mut foreign = HeaderMap::new();
        foreign.insert(header::ORIGIN, "https://attacker.example".parse().unwrap());
        assert!(check_ssh_origin(&state, &foreign).is_err());
        let mut correct = HeaderMap::new();
        correct.insert(header::ORIGIN, "http://localhost".parse().unwrap());
        assert!(check_ssh_origin(&state, &correct).is_ok());
    }

    #[tokio::test]
    async fn ssh_authorization_recheck_observes_session_and_device_revocation() {
        let (state, _receiver) = test_state().await;
        sqlx::query("INSERT INTO devices(id,name,mac,network_profile_id,tailscale_name,revision,ssh_enabled,ssh_username) VALUES('device-1','PC','02:23:45:67:89:ab','test','pc.home.ts.net',1,1,'jack')")
            .execute(&state.db).await.unwrap();
        let token_digest = hex::encode(digest("active-session"));
        sqlx::query(
            "INSERT INTO sessions(token_digest,csrf_token,created_at,expires_at) VALUES(?,?,?,?)",
        )
        .bind(&token_digest)
        .bind("csrf")
        .bind(now())
        .bind(now() + 300)
        .execute(&state.db)
        .await
        .unwrap();
        assert!(ssh_authorization_is_current(
            &state,
            &token_digest,
            "local",
            "device-1",
            "jack",
            "pc.home.ts.net",
        )
        .await
        .unwrap());

        sqlx::query("UPDATE devices SET ssh_enabled=0 WHERE id='device-1'")
            .execute(&state.db)
            .await
            .unwrap();
        assert!(!ssh_authorization_is_current(
            &state,
            &token_digest,
            "local",
            "device-1",
            "jack",
            "pc.home.ts.net",
        )
        .await
        .unwrap());

        sqlx::query("UPDATE devices SET ssh_enabled=1 WHERE id='device-1'")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE token_digest=?")
            .bind(&token_digest)
            .execute(&state.db)
            .await
            .unwrap();
        assert!(!ssh_authorization_is_current(
            &state,
            &token_digest,
            "local",
            "device-1",
            "jack",
            "pc.home.ts.net",
        )
        .await
        .unwrap());
    }

    #[test]
    fn ssh_target_validation_rejects_command_like_values() {
        assert!(valid_ssh_host("pc.home.ts.net"));
        assert!(valid_ssh_host("100.64.2.5"));
        assert!(!valid_ssh_host("--help"));
        assert!(!valid_ssh_host("pc;touch /tmp/pwned"));
        assert!(valid_ssh_username("jack.doe_1"));
        assert!(!valid_ssh_username("jack@example.com"));
    }

    #[tokio::test]
    async fn paired_site_inventory_and_wake_commands_are_scoped_and_revocable() {
        let (state, _receiver) = test_state().await;
        let router = app(state.clone(), None);
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
        let login_body = to_bytes(login.into_body(), usize::MAX).await.unwrap();
        let session: LoginOutput = serde_json::from_slice(&login_body).unwrap();
        let auth_write = |path: String, body: serde_json::Value| {
            Request::post(path)
                .header(header::COOKIE, cookie.clone())
                .header(header::ORIGIN, "http://localhost")
                .header("x-csrf-token", session.csrf_token.clone())
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        let invitation_response = router
            .clone()
            .oneshot(auth_write(
                "/api/v1/connect/invitations".into(),
                serde_json::json!({"site_name":"School"}),
            ))
            .await
            .unwrap();
        assert_eq!(invitation_response.status(), StatusCode::CREATED);
        let invitation: InvitationOutput = serde_json::from_slice(
            &to_bytes(invitation_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let site_id = Uuid::new_v4().to_string();
        let enroll_response=router.clone().oneshot(Request::post("/api/v1/connect/enroll").header("content-type","application/json").body(Body::from(serde_json::json!({"site_id":site_id,"site_name":"School","invitation":invitation.token.clone()}).to_string())).unwrap()).await.unwrap();
        assert_eq!(enroll_response.status(), StatusCode::OK);
        let enrolled: EnrollOutput = serde_json::from_slice(
            &to_bytes(enroll_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let replay = router.clone().oneshot(Request::post("/api/v1/connect/enroll").header("content-type","application/json").body(Body::from(serde_json::json!({"site_id":Uuid::new_v4().to_string(),"site_name":"Replay","invitation":invitation.token}).to_string())).unwrap()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
        let remote_id: String =
            sqlx::query_scalar("SELECT id FROM connect_sites WHERE name='School'")
                .fetch_one(&state.db)
                .await
                .unwrap();
        let long_profile_name = "L".repeat(121);
        let snapshot = serde_json::json!({"site_name":"School","profiles":[{"id":"school-lan","name":long_profile_name}],"devices":[{"id":"school-pc","name":"School PC","mac":"02:23:45:67:89:ab","network_profile_id":"school-lan","network_profile_name":"School LAN","tailscale_name":"school-pc.tailnet.ts.net","ssh_enabled":true,"ssh_username":"admin","revision":1}]});
        let snapshot_response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/connect/snapshot")
                    .header("authorization", format!("Bearer {}", enrolled.credential))
                    .header("content-type", "application/json")
                    .body(Body::from(snapshot.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot_response.status(), StatusCode::NO_CONTENT);
        let stored_profile_name: String = sqlx::query_scalar(
            "SELECT network_profile_name FROM remote_devices WHERE site_id=? AND id='school-pc'",
        )
        .bind(&remote_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(stored_profile_name.len(), 121);

        let sites_response = router
            .clone()
            .oneshot(
                Request::get("/api/v1/sites")
                    .header("authorization", format!("Bearer {}", session.session_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let sites: Vec<Site> = serde_json::from_slice(
            &to_bytes(sites_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(sites
            .iter()
            .any(|s| s.id == remote_id && s.name == "School" && s.online));
        let remote_devices_response = router
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/sites/{remote_id}/devices"))
                    .header("authorization", format!("Bearer {}", session.session_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let remote_devices: Vec<Device> = serde_json::from_slice(
            &to_bytes(remote_devices_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(remote_devices[0].ssh_enabled);
        assert_eq!(remote_devices[0].ssh_username.as_deref(), Some("admin"));
        let mut wake_request = auth_write(
            format!("/api/v1/sites/{remote_id}/devices/school-pc/wake"),
            serde_json::json!({}),
        );
        wake_request
            .headers_mut()
            .insert("idempotency-key", "school-wake-1".parse().unwrap());
        let wake_response = router.clone().oneshot(wake_request).await.unwrap();
        assert_eq!(wake_response.status(), StatusCode::ACCEPTED);
        let queued: WakeOutput = serde_json::from_slice(
            &to_bytes(wake_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(queued.status, "queued");
        let command_response = router
            .clone()
            .oneshot(
                Request::get("/api/v1/connect/commands/next")
                    .header("authorization", format!("Bearer {}", enrolled.credential))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(command_response.status(), StatusCode::OK);
        let command: ConnectCommand = serde_json::from_slice(
            &to_bytes(command_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(command.device_id, "school-pc");
        let duplicate_poll = router
            .clone()
            .oneshot(
                Request::get("/api/v1/connect/commands/next")
                    .header("authorization", format!("Bearer {}", enrolled.credential))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(duplicate_poll.status(), StatusCode::NO_CONTENT);
        let result_response = router
            .clone()
            .oneshot(
                Request::post(format!("/api/v1/connect/commands/{}/result", command.id))
                    .header("authorization", format!("Bearer {}", enrolled.credential))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"sent"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result_response.status(), StatusCode::NO_CONTENT);
        let status_response = router
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/sites/{remote_id}/wake-jobs/{}", queued.id))
                    .header(header::COOKIE, cookie.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status: WakeOutput = serde_json::from_slice(
            &to_bytes(status_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status.status, "sent");

        sqlx::query("UPDATE connect_sites SET last_seen_at=? WHERE id=?")
            .bind(now() - 60)
            .bind(&remote_id)
            .execute(&state.db)
            .await
            .unwrap();
        let mut repeated_wake = auth_write(
            format!("/api/v1/sites/{remote_id}/devices/school-pc/wake"),
            serde_json::json!({}),
        );
        repeated_wake
            .headers_mut()
            .insert("idempotency-key", "school-wake-1".parse().unwrap());
        let repeated = router.clone().oneshot(repeated_wake).await.unwrap();
        assert_eq!(repeated.status(), StatusCode::ACCEPTED);
        let repeated: WakeOutput =
            serde_json::from_slice(&to_bytes(repeated.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(repeated.id, queued.id);
        assert_eq!(repeated.status, "sent");

        let mut offline_request = auth_write(
            format!("/api/v1/sites/{remote_id}/devices/school-pc/wake"),
            serde_json::json!({}),
        );
        offline_request
            .headers_mut()
            .insert("idempotency-key", "offline-wake-1".parse().unwrap());
        let offline = router.clone().oneshot(offline_request).await.unwrap();
        assert_eq!(offline.status(), StatusCode::SERVICE_UNAVAILABLE);

        let mut revoke_request = auth_write(
            format!("/api/v1/connect/sites/{remote_id}"),
            serde_json::json!({}),
        );
        *revoke_request.method_mut() = axum::http::Method::DELETE;
        let revoked = router.clone().oneshot(revoke_request).await.unwrap();
        assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
        let rejected = router
            .oneshot(
                Request::get("/api/v1/connect/commands/next")
                    .header("authorization", format!("Bearer {}", enrolled.credential))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rotated_connect_credential_cannot_poll_or_mutate_site_inventory() {
        let (state, _receiver) = test_state().await;
        let old_credential = format!("wols_{}", "a".repeat(48));
        let new_credential = format!("wols_{}", "b".repeat(48));
        sqlx::query("INSERT INTO connect_sites(id,name,token_digest,is_local,active,last_seen_at,created_at) VALUES('remote','Remote',?,0,1,?,?)")
            .bind(hex::encode(digest(&old_credential)))
            .bind(now())
            .bind(now())
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO remote_devices(site_id,id,name,mac,network_profile_id,network_profile_name,revision) VALUES('remote','pc','Remote PC','02:23:45:67:89:ab','lan','LAN',1)")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO connect_commands(id,site_id,device_id,idempotency_key,status,expires_at,created_at) VALUES('job','remote','pc','key','queued',?,?)")
            .bind(now() + 30)
            .bind(now())
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("UPDATE connect_sites SET token_digest=? WHERE id='remote'")
            .bind(hex::encode(digest(&new_credential)))
            .execute(&state.db)
            .await
            .unwrap();

        let router = app(state.clone(), None);
        let poll = router
            .clone()
            .oneshot(
                Request::get("/api/v1/connect/commands/next")
                    .header("authorization", format!("Bearer {old_credential}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll.status(), StatusCode::UNAUTHORIZED);
        let result = router
            .clone()
            .oneshot(
                Request::post("/api/v1/connect/commands/job/result")
                    .header("authorization", format!("Bearer {old_credential}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status":"sent"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::UNAUTHORIZED);
        let snapshot = router
            .oneshot(
                Request::post("/api/v1/connect/snapshot")
                    .header("authorization", format!("Bearer {old_credential}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"site_name":"Overwritten","profiles":[],"devices":[]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.status(), StatusCode::UNAUTHORIZED);
        let status: String =
            sqlx::query_scalar("SELECT status FROM connect_commands WHERE id='job'")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(status, "queued");
        let name: String = sqlx::query_scalar("SELECT name FROM connect_sites WHERE id='remote'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(name, "Remote");
    }

    #[tokio::test]
    async fn connector_credential_is_published_atomically_and_never_overwritten() {
        let directory = std::env::temp_dir().join(format!("wolui-credential-{}", Uuid::new_v4()));
        let path = directory.join("connect-token");
        let credential = format!("wols_{}", "a".repeat(48));
        persist_connector_credential(&path, &credential)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), credential);
        assert!(
            persist_connector_credential(&path, &format!("wols_{}", "b".repeat(48)))
                .await
                .is_err()
        );
        assert!(persist_connector_credential(&directory.join("invalid"), "")
            .await
            .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn expired_connect_command_does_not_send_a_magic_packet() {
        let (state, receiver) = test_state().await;
        sqlx::query("INSERT INTO devices(id,name,mac,network_profile_id,revision) VALUES('device','PC','02:23:45:67:89:ab','test',1)")
            .execute(&state.db)
            .await
            .unwrap();
        let result = dispatch_local_wake(&state, "device", "expired-connect-job", Some(now() - 1))
            .await
            .unwrap();
        assert_eq!(result.status, "unknown");
        let stored_status: String = sqlx::query_scalar(
            "SELECT status FROM wake_jobs WHERE idempotency_key='expired-connect-job'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(stored_status, "unknown");
        receiver.set_nonblocking(true).unwrap();
        let mut packet = [0u8; 102];
        assert!(receiver.recv_from(&mut packet).is_err());
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
                || p.name.len() > MAX_PROFILE_NAME_BYTES
                || p.interface.trim().is_empty()
                || p.port == 0
            {
                return Err(
                    "invalid WOLUI_NETWORK_PROFILES entry (name must be 1 to 256 bytes)".into(),
                );
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
    let site_name = env::var("WOLUI_SITE_NAME").unwrap_or_else(|_| "Local site".into());
    if site_name.trim().is_empty() || site_name.len() > 80 {
        return Err("WOLUI_SITE_NAME must contain 1 to 80 characters".into());
    }
    let public_origin = env::var("WOLUI_PUBLIC_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string());
    let state = AppState {
        db,
        site_id,
        site_name,
        role,
        access_digest: digest(access_token),
        secure_cookie: !allow_dev,
        public_origin,
        ssh_gateway_enabled: matches!(env::var("WOLUI_SSH_ENABLED").as_deref(), Ok("1" | "true")),
        tailscale_bin: env::var("WOLUI_TAILSCALE_BIN")
            .unwrap_or_else(|_| "/usr/local/bin/tailscale".into()),
        tailscale_socket: env::var("WOLUI_TAILSCALE_SOCKET")
            .unwrap_or_else(|_| "/var/run/tailscale/tailscaled.sock".into()),
        ssh_sessions: Arc::new(Semaphore::new(4)),
    };
    if let Some(connect_url) = env::var("WOLUI_CONNECT_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let invitation_file = env::var("WOLUI_CONNECT_INVITATION_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&data).join("connect-invitation"));
        let credential_file = env::var("WOLUI_CONNECT_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&data).join("connect-token"));
        tokio::spawn(run_connector(
            state.clone(),
            connect_url,
            invitation_file,
            credential_file,
        ));
    }
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
