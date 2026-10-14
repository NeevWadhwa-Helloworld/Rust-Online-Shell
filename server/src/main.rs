use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        RawQuery,
    },
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,server=info".to_string()),
        )
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let dist_dir = find_dist_dir();
    tracing::info!("Serving frontend assets from: {:?}", dist_dir);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "OK" }))
        .fallback_service(ServeDir::new(&dist_dir).fallback(get(fallback_page)))
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|_| panic!("Failed to bind to {}", addr));

    tracing::info!("Server running on http://0.0.0.0:{}", port);

    axum::serve(listener, app)
        .await
        .expect("Server failed");
}

async fn fallback_page() -> Html<&'static str> {
    Html(include_str!("../../frontend/index.html"))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    RawQuery(query): RawQuery,
) -> impl IntoResponse {
    let token = extract_token(query.as_deref());
    ws.on_upgrade(move |socket| handle_socket(socket, token))
}

fn extract_token(query: Option<&str>) -> String {
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "token" && !v.is_empty() {
                    let is_safe = v.len() >= 8
                        && v.len() <= 64
                        && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
                    if is_safe {
                        return v.to_string();
                    }
                }
            }
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("guest-{}", now)
}

fn get_user_home_dir(token: &str) -> PathBuf {
    let base_dir = if let Ok(dir) = std::env::var("STORAGE_DIR") {
        PathBuf::from(dir)
    } else {
        let shuttle_storage = PathBuf::from("/shuttle/storage");
        if shuttle_storage.exists() && shuttle_storage.is_dir() {
            shuttle_storage.join("users")
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("storage")
                .join("users")
        }
    };

    let user_home = base_dir.join(token).join("home");
    let _ = std::fs::create_dir_all(&user_home);
    user_home
}

async fn handle_socket(socket: WebSocket, token: String) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let shell_path = find_shell_binary();
    let user_home = get_user_home_dir(&token);
    tracing::info!("Spawning shell session for [{}] in {:?}", token, user_home);

    let mut child = match Command::new(&shell_path)
        .current_dir(&user_home)
        .env("HOME", &user_home)
        .env("USER_ID", &token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            let msg = format!("Failed to start shell: {}\r\n", err);
            tracing::error!("{}", msg);
            let _ = ws_sender.send(Message::Text(msg)).await;
            return;
        }
    };

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut stdout = child.stdout.take().expect("Failed to open stdout");
    let mut stderr = child.stderr.take().expect("Failed to open stderr");

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(100);

    let socket_writer_task = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_sender.send(Message::Text(chunk)).await.is_err() {
                break;
            }
        }
    });

    let tx_stdout = out_tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            match stdout.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buffer[..n]).to_string();
                    if tx_stdout.send(normalize_crlf(&text)).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let tx_stderr = out_tx;
    let stderr_task = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buffer[..n]).to_string();
                    if tx_stderr.send(normalize_crlf(&text)).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let stdin_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if stdin.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                    let _ = stdin.flush().await;
                }
                Message::Binary(bin) => {
                    if stdin.write_all(&bin).await.is_err() {
                        break;
                    }
                    let _ = stdin.flush().await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = socket_writer_task => {},
        _ = stdout_task => {},
        _ = stderr_task => {},
        _ = stdin_task => {},
        status = child.wait() => {
            tracing::info!("Shell exited: {:?}", status);
        }
    }

    let _ = child.kill().await;
}

fn find_dist_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("dist"),
        PathBuf::from("frontend/dist"),
        PathBuf::from("../dist"),
        PathBuf::from("../frontend/dist"),
    ];
    for c in &candidates {
        if c.exists() && c.is_dir() {
            return c.clone();
        }
    }
    PathBuf::from("dist")
}

fn find_shell_binary() -> PathBuf {
    let exe_name = if cfg!(windows) { "rust_shell.exe" } else { "rust_shell" };

    if let Ok(explicit) = std::env::var("SHELL_BINARY_PATH") {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return p;
        }
    }

    let mut candidates = vec![
        PathBuf::from(format!("target/release/{}", exe_name)),
        PathBuf::from(format!("target/debug/{}", exe_name)),
        PathBuf::from(format!("../target/release/{}", exe_name)),
        PathBuf::from(format!("../target/debug/{}", exe_name)),
    ];

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            candidates.push(parent.join(exe_name));
        }
    }

    for c in &candidates {
        if c.exists() && c.is_file() {
            return c.clone();
        }
    }

    PathBuf::from(exe_name)
}

fn normalize_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}
