use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use flint_core::{Algorithm, Limiter, MultiCheckItem, TopBy};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Parser)]
#[command(name = "flint", about = "embedded persistent rate limiter")]
struct Cli {
    #[arg(long, global = true, default_value = ".flint")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Limit {
        #[command(subcommand)]
        command: LimitCommand,
    },
    Log {
        #[command(subcommand)]
        command: LogCommand,
    },
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    Doctor,
}

#[derive(Subcommand)]
enum LimitCommand {
    Add {
        key: String,
        #[arg(long)]
        rate: u64,
        #[arg(long)]
        per: String,
        #[arg(long, default_value = "token_bucket")]
        algorithm: String,
    },
    List,
    Status {
        key: String,
    },
    Check {
        key: String,
        #[arg(long, default_value_t = 1)]
        cost: u64,
    },
    CheckAll {
        keys: Vec<String>,
        #[arg(long = "cost")]
        costs: Vec<String>,
    },
    Reset {
        key: String,
    },
    History {
        key: String,
    },
    Top {
        #[arg(long, default_value = "denied")]
        by: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum LogCommand {
    Compact,
}

#[derive(Subcommand)]
enum ServerCommand {
    Start {
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: SocketAddr,
        #[arg(long, env = "FLINT_SERVER_TOKEN")]
        token: Option<String>,
        #[arg(long, default_value_t = 128)]
        max_blocking: usize,
    },
}

#[derive(Clone)]
struct AppState {
    limiter: Arc<Limiter>,
    token: Option<String>,
    blocking_permits: Arc<Semaphore>,
}

#[derive(Debug, Deserialize)]
struct ConfigureLimitRequest {
    key: String,
    rate: u64,
    per: String,
    #[serde(default = "default_algorithm")]
    algorithm: String,
}

#[derive(Debug, Deserialize)]
struct CheckRequest {
    key: String,
    #[serde(default = "default_cost")]
    cost: u64,
}

#[derive(Debug, Deserialize)]
struct CheckAllRequest {
    items: Vec<MultiCheckItem>,
}

#[derive(Debug, Deserialize)]
struct ResetRequest {
    key: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli).await {
        eprintln!("flint: {err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let limiter = Limiter::open(&cli.data_dir)?;
    match cli.command {
        Command::Limit { command } => match command {
            LimitCommand::Add {
                key,
                rate,
                per,
                algorithm,
            } => {
                limiter.limit(key, rate, per, Algorithm::parse(&algorithm)?)?;
                println!("configured");
            }
            LimitCommand::List => {
                println!("{}", serde_json::to_string_pretty(&limiter.list()?)?);
            }
            LimitCommand::Status { key } => {
                println!("{}", serde_json::to_string_pretty(&limiter.status(&key)?)?);
            }
            LimitCommand::Check { key, cost } => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&limiter.check_cost(&key, cost)?)?
                );
            }
            LimitCommand::CheckAll { keys, costs } => {
                let items = multi_items_from_cli(keys, costs)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&limiter.check_all(items)?)?
                );
            }
            LimitCommand::Reset { key } => {
                limiter.reset(&key)?;
                println!("reset");
            }
            LimitCommand::History { key } => {
                println!("{}", serde_json::to_string_pretty(&limiter.history(&key)?)?);
            }
            LimitCommand::Top { by, limit } => {
                let by = match by.as_str() {
                    "allowed" => TopBy::Allowed,
                    "denied" => TopBy::Denied,
                    other => return Err(format!("unsupported top selector: {other}").into()),
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&limiter.top(by, limit)?)?
                );
            }
        },
        Command::Log { command } => match command {
            LogCommand::Compact => {
                limiter.compact()?;
                println!("compacted");
            }
        },
        Command::Server { command } => match command {
            ServerCommand::Start {
                bind,
                token,
                max_blocking,
            } => {
                run_server(limiter, bind, token, max_blocking).await?;
            }
        },
        Command::Doctor => {
            println!("{}", serde_json::to_string_pretty(&limiter.doctor()?)?);
        }
    }
    Ok(())
}

fn multi_items_from_cli(
    keys: Vec<String>,
    costs: Vec<String>,
) -> Result<Vec<MultiCheckItem>, Box<dyn std::error::Error>> {
    let mut cost_by_key = std::collections::HashMap::new();
    for cost in costs {
        let Some((key, value)) = cost.split_once('=') else {
            return Err(format!("invalid --cost value {cost:?}; expected key=cost").into());
        };
        let parsed = value.parse::<u64>()?;
        if cost_by_key.insert(key.to_string(), parsed).is_some() {
            return Err(format!("duplicate --cost specified for key: {key}").into());
        }
    }
    let items = keys
        .into_iter()
        .map(|key| {
            let cost = cost_by_key.remove(&key).unwrap_or(1);
            MultiCheckItem { key, cost }
        })
        .collect::<Vec<_>>();
    if !cost_by_key.is_empty() {
        let unknown = cost_by_key.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(format!("--cost specified for unknown limit key(s): {unknown}").into());
    }
    Ok(items)
}

async fn run_server(
    limiter: Limiter,
    bind: SocketAddr,
    token: Option<String>,
    max_blocking: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if token.is_none() && !bind.ip().is_loopback() {
        return Err(
            "refusing to bind shared server to a non-loopback address without --token".into(),
        );
    }
    if max_blocking == 0 {
        return Err("--max-blocking must be greater than zero".into());
    }
    let state = AppState {
        limiter: Arc::new(limiter),
        token,
        blocking_permits: Arc::new(Semaphore::new(max_blocking)),
    };
    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/limits", get(list_limits).post(configure_limit))
        .route("/v1/limits/:key", get(limit_status))
        .route("/v1/check", post(check_limit))
        .route("/v1/check-all", post(check_all))
        .route("/v1/reset", post(reset_limit))
        .route("/v1/log/compact", post(compact_log))
        .route("/v1/doctor", get(doctor))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    let local_addr = listener.local_addr()?;
    println!("flint server listening on http://{local_addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn health(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    Ok(Json(json!({ "ok": true })))
}

async fn configure_limit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ConfigureLimitRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    blocking_core(&state, move || {
        let algorithm = Algorithm::parse(&request.algorithm)?;
        limiter.limit(request.key, request.rate, request.per, algorithm)
    })
    .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_limits(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    to_json(blocking_core(&state, move || limiter.list()).await?)
}

async fn limit_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    to_json(blocking_core(&state, move || limiter.status(&key)).await?)
}

async fn check_limit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CheckRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    to_json(
        blocking_core(&state, move || {
            limiter.check_cost(&request.key, request.cost)
        })
        .await?,
    )
}

async fn check_all(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CheckAllRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    to_json(blocking_core(&state, move || limiter.check_all(request.items)).await?)
}

async fn reset_limit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ResetRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    blocking_core(&state, move || limiter.reset(&request.key)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn compact_log(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    blocking_core(&state, move || limiter.compact()).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn doctor(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize(&state, &headers)?;
    let limiter = Arc::clone(&state.limiter);
    to_json(blocking_core(&state, move || limiter.doctor()).await?)
}

async fn blocking_core<T, F>(
    state: &AppState,
    operation: F,
) -> Result<T, (StatusCode, Json<ErrorResponse>)>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, flint_core::FlintError> + Send + 'static,
{
    let permit = state
        .blocking_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|err| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: format!("server is shutting down: {err}"),
                }),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _permit: OwnedSemaphorePermit = permit;
        operation()
    })
    .await
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("blocking task failed: {err}"),
            }),
        )
    })?
    .map_err(api_error)
}

fn authorize(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(expected) = &state.token else {
        return Ok(());
    };
    let provided = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if provided == Some(expected.as_str()) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "unauthorized".to_string(),
            }),
        ))
    }
}

fn to_json<T: Serialize>(
    value: T,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    serde_json::to_value(value).map(Json).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
    })
}

fn api_error(err: flint_core::FlintError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match err {
        flint_core::FlintError::InvalidDuration(_)
        | flint_core::FlintError::UnsupportedAlgorithm(_)
        | flint_core::FlintError::LimitNotConfigured(_) => StatusCode::BAD_REQUEST,
        flint_core::FlintError::DataDirLocked { .. } => StatusCode::CONFLICT,
        flint_core::FlintError::UnsupportedSnapshot(_)
        | flint_core::FlintError::CorruptLog { .. }
        | flint_core::FlintError::StorageIntegrity(_) => StatusCode::INTERNAL_SERVER_ERROR,
        flint_core::FlintError::Io(_) | flint_core::FlintError::Json(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        Json(ErrorResponse {
            error: err.to_string(),
        }),
    )
}

fn default_algorithm() -> String {
    "token_bucket".to_string()
}

fn default_cost() -> u64 {
    1
}
