//! Loopback-only validation UI for the real native transfer engine.
//! Run with a disposable fixture directory and data directory on the command line.
use anyhow::{bail, Result};
use irohcore::{Core, CoreConfig, Progress};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_stream::StreamExt;

#[derive(Default)]
struct State {
    send_ticket: String,
    preview_ticket: String,
    status: String,
    manifest: String,
    csrf: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        bail!("Usage: browser_peer FIXTURES DATA_DIR SAVE_DIR");
    }
    let files = PathBuf::from(&args[0]);
    let core = Core::start(CoreConfig::serverless(&args[1])).await?;
    let state = Arc::new(Mutex::new(State {
        status: "Starting native sender…".into(),
        csrf: uuid::Uuid::new_v4().to_string(),
        ..Default::default()
    }));
    let (_, mut events) = core.send(files).await?;
    while let Some(event) = events.next().await {
        match event {
            Progress::Ready { ticket, .. } => {
                state.lock().unwrap().send_ticket = ticket;
                break;
            }
            Progress::Error { message, .. } => bail!("{message}"),
            _ => {}
        }
    }
    state.lock().unwrap().status =
        "Native sender ready. The ticket is shown only in this local validation UI.".into();
    tokio::spawn(async move { while events.next().await.is_some() {} });
    let listener = TcpListener::bind("127.0.0.1:8091").await?;
    println!("Native validation endpoint: http://127.0.0.1:8091");
    loop {
        let (socket, _) = listener.accept().await?;
        let core = core.clone();
        let state = state.clone();
        let dest = PathBuf::from(&args[2]);
        tokio::spawn(async move {
            if let Err(error) = serve(socket, core, state, dest).await {
                eprintln!("Validation request failed: {error}");
            }
        });
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn serve(
    mut socket: TcpStream,
    core: Core,
    state: Arc<Mutex<State>>,
    dest: PathBuf,
) -> Result<()> {
    let mut data = Vec::new();
    let mut buffer = [0; 2048];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer)).await??;
        if n == 0 {
            return Ok(());
        }
        data.extend_from_slice(&buffer[..n]);
        if data.len() > 16384 {
            bail!("request too large");
        }
        if let Some(end) = data.windows(4).position(|s| s == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&data[..header_end]).to_string();
    let host = headers.lines().find_map(|line| {
        line.split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim())
    });
    if host != Some("127.0.0.1:8091") {
        bail!("invalid host");
    }
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if length > 8192 {
        bail!("body too large");
    }
    while data.len() < header_end + length {
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer)).await??;
        if n == 0 {
            bail!("incomplete request");
        }
        data.extend_from_slice(&buffer[..n]);
    }
    if headers.starts_with("POST ") {
        let csrf = url::form_urlencoded::parse(&data[header_end..header_end + length])
            .find(|(key, _)| key == "csrf")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        if csrf != state.lock().unwrap().csrf {
            bail!("invalid request token");
        }
    }
    if headers.starts_with("POST /preview ") {
        let ticket = url::form_urlencoded::parse(&data[header_end..header_end + length])
            .find(|(key, _)| key == "code")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        match core.inspect(ticket.clone()).await {
            Ok(preview) => {
                let mut s = state.lock().unwrap();
                s.preview_ticket = ticket;
                s.manifest = preview
                    .files
                    .iter()
                    .map(|f| format!("<li>{} · {} bytes</li>", escape(&f.name), f.size))
                    .collect();
                s.status = format!(
                    "Native preview verified: {} files, {} bytes. Explicit acceptance is required.",
                    preview.file_count, preview.total_bytes
                );
            }
            Err(_) => {
                state.lock().unwrap().status =
                    "Native preview failed; code invalid, unavailable, or already claimed.".into();
            }
        }
    }
    if headers.starts_with("POST /accept ") {
        let ticket = state.lock().unwrap().preview_ticket.clone();
        if !ticket.is_empty() {
            let (_, mut events) = core.receive(ticket, dest).await?;
            while let Some(event) = events.next().await {
                match event {
                    Progress::Done { stats, .. } => {
                        state.lock().unwrap().status = format!(
                            "Native receive complete: {} bytes verified and saved.",
                            stats.bytes
                        );
                        break;
                    }
                    Progress::Error { message, .. } => {
                        state.lock().unwrap().status = message;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    if headers.starts_with("POST ") {
        socket.write_all(b"HTTP/1.1 303 See Other\r\nLocation: /\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(());
    }
    let html = {
        let s = state.lock().unwrap();
        format!(
            r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="referrer" content="no-referrer"><title>Native peer validation</title><style>body{{font:16px system-ui;background:#0e1116;color:#eef1e8;margin:40px;max-width:850px}}textarea{{display:block;width:100%;height:110px}}button{{margin:16px 0;padding:12px 20px}}</style><h1>Native peer validation</h1><p role="status">{}</p><h2>Native → browser</h2><label for="native-code">Native sender code</label><textarea id="native-code" readonly>{}</textarea><h2>Browser → native</h2><form method="post" action="/preview"><input type="hidden" name="csrf" value="{}"><label for="code">Browser sender code</label><textarea name="code" id="code" autocomplete="off"></textarea><button>Preview in native engine</button></form><ul>{}</ul><form method="post" action="/accept"><input type="hidden" name="csrf" value="{}"><button>Accept and save in native engine</button></form><p>This is a test fixture on 127.0.0.1. File bytes use the real Dropwire protocol, not HTTP.</p></html>"#,
            escape(&s.status),
            escape(&s.send_ticket),
            s.csrf,
            s.manifest,
            s.csrf
        )
    };
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", html.len(), html);
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}
