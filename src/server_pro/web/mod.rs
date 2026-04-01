//! Web dashboard module for btest-server-pro.
//!
//! Provides an axum-based HTTP dashboard with:
//! - Landing page with IP lookup
//! - Per-IP session history and statistics
//! - Chart.js throughput graphs
//!
//! # Feature gate
//!
//! This entire module is compiled only when the `pro` feature is active
//! (it lives inside the `btest-server-pro` binary crate which already
//! requires `--features pro`).
//!
//! # Template files
//!
//! The HTML source lives in `src/server_pro/web/templates/` as standalone
//! `.html` files for easy editing. The Rust code embeds them via the askama
//! `source` attribute so no `askama.toml` configuration is needed. If you
//! prefer external template files, create `askama.toml` at the crate root:
//!
//! ```toml
//! [[dirs]]
//! path = "src/server_pro/web/templates"
//! ```
//!
//! Then change `source = "..."` to `path = "index.html"` (etc.) in the
//! template structs below.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rusqlite::{params, Connection};
use serde::Serialize;

use super::user_db::UserDb;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Shared application state passed to all handlers via axum's `State`.
pub struct WebState {
    /// Reference to the main user/session database.
    pub db: UserDb,
    /// Separate read-only connection for dashboard queries that are not
    /// exposed by [`UserDb`] (e.g. listing sessions, aggregate stats).
    /// Wrapped in a [`std::sync::Mutex`] because [`rusqlite::Connection`]
    /// is not `Send + Sync` on its own.
    pub query_conn: std::sync::Mutex<Connection>,
}

// ---------------------------------------------------------------------------
// Router constructor
// ---------------------------------------------------------------------------

/// Default database filename used when `BTEST_DB_PATH` is not set.
const DEFAULT_DB_PATH: &str = "btest-users.db";

/// Build the axum [`Router`] for the web dashboard.
///
/// The database path for the read-only query connection is resolved in the
/// following order:
///
/// 1. The `BTEST_DB_PATH` environment variable (if set).
/// 2. The compile-time default `btest-users.db`.
///
/// # Panics
///
/// Panics if the read-only database connection or the DDL for the
/// `session_intervals` table cannot be established. This is intentional:
/// the web module is optional and failure during startup should surface
/// loudly rather than silently serving broken pages.
pub fn create_router(db: UserDb) -> Router {
    let db_path = std::env::var("BTEST_DB_PATH").unwrap_or_else(|_| DEFAULT_DB_PATH.to_string());

    let query_conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("web: failed to open read-only database connection");
    query_conn
        .execute_batch("PRAGMA busy_timeout=5000;")
        .expect("web: failed to set PRAGMA on query connection");

    // Ensure the `session_intervals` table exists. The server loop must
    // INSERT rows for the chart to have data; the table is created here so
    // the schema is ready.
    ensure_web_tables(&db_path).expect("web: failed to create session_intervals table");

    let state = Arc::new(WebState {
        db,
        query_conn: std::sync::Mutex::new(query_conn),
    });

    // axum 0.8 uses `{param}` syntax for path parameters.
    Router::new()
        .route("/", get(index_page))
        .route("/dashboard/{ip}", get(dashboard_page))
        .route("/api/ip/{ip}/sessions", get(api_sessions))
        .route("/api/ip/{ip}/stats", get(api_stats))
        .route("/api/session/{id}/intervals", get(api_intervals))
        .with_state(state)
}

/// Create additional tables the web dashboard depends on.
///
/// Opens a short-lived writable connection solely for DDL so it does not
/// interfere with the main [`UserDb`] connection.
fn ensure_web_tables(db_path: &str) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_intervals (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER NOT NULL,
            second      INTEGER NOT NULL,
            tx_bytes    INTEGER NOT NULL DEFAULT 0,
            rx_bytes    INTEGER NOT NULL DEFAULT 0,
            UNIQUE(session_id, second)
        );
        CREATE INDEX IF NOT EXISTS idx_intervals_session
            ON session_intervals(session_id, second);",
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Askama templates (embedded via `source`)
// ---------------------------------------------------------------------------

/// Landing / index page template.
#[derive(Template)]
#[template(
    source = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>btest-rs Public Bandwidth Test Server</title>
<style>
  *{margin:0;padding:0;box-sizing:border-box}
  body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:#0f1117;color:#e1e4e8;min-height:100vh;display:flex;flex-direction:column;align-items:center;justify-content:center}
  .container{max-width:560px;width:90%;text-align:center;padding:2rem}
  h1{font-size:2rem;margin-bottom:.5rem;color:#58a6ff}
  .subtitle{color:#8b949e;margin-bottom:2rem;line-height:1.5}
  .search-box{display:flex;gap:.5rem;margin-bottom:1.5rem}
  .search-box input{flex:1;padding:.75rem 1rem;border:1px solid #30363d;border-radius:6px;background:#161b22;color:#e1e4e8;font-size:1rem;outline:none}
  .search-box input:focus{border-color:#58a6ff}
  .search-box input::placeholder{color:#484f58}
  .search-box button{padding:.75rem 1.5rem;background:#238636;color:#fff;border:none;border-radius:6px;font-size:1rem;cursor:pointer;white-space:nowrap}
  .search-box button:hover{background:#2ea043}
  .info{background:#161b22;border:1px solid #30363d;border-radius:8px;padding:1.5rem;text-align:left;line-height:1.6;color:#8b949e}
  .info h3{color:#e1e4e8;margin-bottom:.5rem}
  .info code{background:#0d1117;padding:.15rem .4rem;border-radius:4px;font-size:.9em;color:#58a6ff}
  .auto-link{margin-top:1rem;font-size:.9rem}
  .auto-link a{color:#58a6ff;text-decoration:none}
  .auto-link a:hover{text-decoration:underline}
  .footer{margin-top:2rem;color:#484f58;font-size:.8rem}
</style>
</head>
<body>
<div class="container">
  <h1>btest-rs</h1>
  <p class="subtitle">Public MikroTik Bandwidth Test Server &mdash; view your test results and history.</p>
  <form class="search-box" id="ip-form" onsubmit="return goToDashboard()">
    <input type="text" id="ip-input" placeholder="Enter your IP address (e.g. 203.0.113.5)" autocomplete="off">
    <button type="submit">View Results</button>
  </form>
  <div class="auto-link" id="auto-detect">Detecting your IP...</div>
  <div class="info">
    <h3>How it works</h3>
    <p>Run a bandwidth test from your MikroTik router targeting this server.
       After the test completes, enter your public IP above to see
       throughput charts, session history, and aggregate statistics.</p>
    <p style="margin-top:0.5rem">
      Example: <code>/tool bandwidth-test address=this-server protocol=tcp direction=both</code>
    </p>
  </div>
  <div class="footer">Powered by btest-rs</div>
</div>
<script>
function goToDashboard(){var ip=document.getElementById('ip-input').value.trim();if(ip){window.location.href='/dashboard/'+encodeURIComponent(ip);}return false;}
fetch('https://api.ipify.org?format=json')
  .then(function(r){return r.json();})
  .then(function(d){if(d.ip){document.getElementById('ip-input').value=d.ip;document.getElementById('auto-detect').innerHTML='Detected IP: <a href="/dashboard/'+encodeURIComponent(d.ip)+'">'+d.ip+'</a> &mdash; click to view your dashboard';}})
  .catch(function(){document.getElementById('auto-detect').textContent='';});
</script>
</body>
</html>"##,
    ext = "html"
)]
struct IndexTemplate;

/// Per-IP dashboard page template.
#[derive(Template)]
#[template(
    source = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Dashboard &mdash; {{ ip }} &mdash; btest-rs</title>
<style>
  *{margin:0;padding:0;box-sizing:border-box}
  body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:#0f1117;color:#e1e4e8;min-height:100vh;padding:1.5rem}
  a{color:#58a6ff;text-decoration:none}a:hover{text-decoration:underline}
  .header{display:flex;align-items:center;gap:1rem;margin-bottom:1.5rem;flex-wrap:wrap}
  .header h1{font-size:1.5rem;color:#58a6ff}
  .header .ip-label{font-size:1.1rem;color:#8b949e;font-family:monospace}
  .header .home-link{margin-left:auto}
  .stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:1rem;margin-bottom:1.5rem}
  .stat-card{background:#161b22;border:1px solid #30363d;border-radius:8px;padding:1rem}
  .stat-card .label{color:#8b949e;font-size:.8rem;text-transform:uppercase;letter-spacing:.05em}
  .stat-card .value{font-size:1.4rem;font-weight:600;margin-top:.25rem}
  .table-wrap{overflow-x:auto;margin-bottom:1.5rem}
  table{width:100%;border-collapse:collapse;background:#161b22;border-radius:8px;overflow:hidden}
  th,td{padding:.6rem 1rem;text-align:left;border-bottom:1px solid #21262d;white-space:nowrap}
  th{background:#0d1117;color:#8b949e;font-size:.8rem;text-transform:uppercase;letter-spacing:.04em}
  tr{cursor:pointer}tr:hover td{background:#1c2128}tr.selected td{background:#1f3a5f}
  .proto-tcp{color:#3fb950}.proto-udp{color:#d29922}
  .dir-tx{color:#f78166}.dir-rx{color:#58a6ff}.dir-both{color:#bc8cff}
  .chart-section{background:#161b22;border:1px solid #30363d;border-radius:8px;padding:1.5rem;margin-bottom:1.5rem}
  .chart-section h2{font-size:1rem;color:#8b949e;margin-bottom:1rem}
  .chart-container{position:relative;width:100%;max-height:360px}
  .chart-placeholder{text-align:center;color:#484f58;padding:3rem 0}
  .footer{text-align:center;color:#484f58;font-size:.8rem;margin-top:2rem}
  .no-data{text-align:center;padding:3rem;color:#484f58}
</style>
</head>
<body>
<div class="header">
  <h1>btest-rs</h1>
  <span class="ip-label">{{ ip }}</span>
  <span class="home-link"><a href="/">Home</a></span>
</div>
<div class="stats" id="stats-grid">
  <div class="stat-card"><div class="label">Total Tests</div><div class="value" id="stat-total-tests">&mdash;</div></div>
  <div class="stat-card"><div class="label">Total TX</div><div class="value" id="stat-total-tx">&mdash;</div></div>
  <div class="stat-card"><div class="label">Total RX</div><div class="value" id="stat-total-rx">&mdash;</div></div>
  <div class="stat-card"><div class="label">Avg TX Mbps</div><div class="value" id="stat-avg-tx">&mdash;</div></div>
  <div class="stat-card"><div class="label">Avg RX Mbps</div><div class="value" id="stat-avg-rx">&mdash;</div></div>
</div>
<div class="chart-section">
  <h2 id="chart-title">Select a test below to view its throughput chart</h2>
  <div class="chart-container">
    <canvas id="throughput-chart"></canvas>
    <div class="chart-placeholder" id="chart-placeholder">Click a row in the table to load the throughput graph for that session.</div>
  </div>
</div>
<div class="table-wrap">
  <table>
    <thead><tr><th>#</th><th>Date</th><th>Protocol</th><th>Direction</th><th>TX Bytes</th><th>RX Bytes</th><th>Duration</th><th>Avg TX Mbps</th><th>Avg RX Mbps</th></tr></thead>
    <tbody id="sessions-body"><tr><td colspan="9" class="no-data">Loading sessions...</td></tr></tbody>
  </table>
</div>
<div class="footer">Powered by btest-rs</div>
<script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
<script>
var currentIp="{{ ip }}";
var throughputChart=null;
function formatBytes(b){if(b===0)return'0 B';var u=['B','KB','MB','GB','TB'];var i=Math.floor(Math.log(b)/Math.log(1024));if(i>=u.length)i=u.length-1;return(b/Math.pow(1024,i)).toFixed(1)+' '+u[i];}
function formatMbps(bps){return(bps*8/1e6).toFixed(2);}
function durationStr(s,e){if(!s||!e)return'--';var ms=new Date(e)-new Date(s);if(ms<0)return'--';var sec=Math.round(ms/1000);if(sec<60)return sec+'s';return Math.floor(sec/60)+'m '+(sec%60)+'s';}
function durationSec(s,e){if(!s||!e)return 0;return Math.max((new Date(e)-new Date(s))/1000,0.001);}
fetch('/api/ip/'+encodeURIComponent(currentIp)+'/stats').then(function(r){return r.json();}).then(function(d){
  document.getElementById('stat-total-tests').textContent=d.total_sessions||0;
  document.getElementById('stat-total-tx').textContent=formatBytes(d.total_tx_bytes||0);
  document.getElementById('stat-total-rx').textContent=formatBytes(d.total_rx_bytes||0);
  document.getElementById('stat-avg-tx').textContent=d.avg_tx_mbps?d.avg_tx_mbps.toFixed(2):'0.00';
  document.getElementById('stat-avg-rx').textContent=d.avg_rx_mbps?d.avg_rx_mbps.toFixed(2):'0.00';
}).catch(function(){});
fetch('/api/ip/'+encodeURIComponent(currentIp)+'/sessions').then(function(r){return r.json();}).then(function(sessions){
  var tbody=document.getElementById('sessions-body');
  if(!sessions||sessions.length===0){tbody.innerHTML='<tr><td colspan="9" class="no-data">No test sessions found for this IP.</td></tr>';return;}
  tbody.innerHTML='';
  sessions.forEach(function(s,i){
    var tr=document.createElement('tr');tr.dataset.sessionId=s.id;tr.onclick=function(){selectSession(s.id,tr);};
    var dur=durationSec(s.started_at,s.ended_at);var avgTx=dur>0?formatMbps(s.tx_bytes/dur):'0.00';var avgRx=dur>0?formatMbps(s.rx_bytes/dur):'0.00';
    var proto=(s.protocol||'TCP').toUpperCase();var dir=(s.direction||'BOTH').toUpperCase();
    var pc=proto==='UDP'?'proto-udp':'proto-tcp';var dc=dir==='TX'?'dir-tx':dir==='RX'?'dir-rx':'dir-both';
    tr.innerHTML='<td>'+(i+1)+'</td><td>'+(s.started_at||'--')+'</td><td class="'+pc+'">'+proto+'</td><td class="'+dc+'">'+dir+'</td><td>'+formatBytes(s.tx_bytes||0)+'</td><td>'+formatBytes(s.rx_bytes||0)+'</td><td>'+durationStr(s.started_at,s.ended_at)+'</td><td>'+avgTx+'</td><td>'+avgRx+'</td>';
    tbody.appendChild(tr);
  });
  if(sessions.length>0){var fr=tbody.querySelector('tr');if(fr)selectSession(sessions[0].id,fr);}
}).catch(function(){document.getElementById('sessions-body').innerHTML='<tr><td colspan="9" class="no-data">Failed to load sessions.</td></tr>';});
function selectSession(sid,row){
  document.querySelectorAll('#sessions-body tr').forEach(function(r){r.classList.remove('selected');});
  row.classList.add('selected');
  document.getElementById('chart-title').textContent='Throughput for session #'+sid;
  document.getElementById('chart-placeholder').style.display='none';
  fetch('/api/session/'+sid+'/intervals').then(function(r){return r.json();}).then(function(iv){renderChart(iv);}).catch(function(){
    document.getElementById('chart-placeholder').style.display='block';
    document.getElementById('chart-placeholder').textContent='Failed to load interval data.';
  });
}
function renderChart(iv){
  var canvas=document.getElementById('throughput-chart');
  if(throughputChart)throughputChart.destroy();
  if(!iv||iv.length===0){document.getElementById('chart-placeholder').style.display='block';document.getElementById('chart-placeholder').textContent='No interval data available for this session.';return;}
  var labels=iv.map(function(d){return d.second+'s';});
  var tx=iv.map(function(d){return(d.tx_bytes*8/1e6).toFixed(2);});
  var rx=iv.map(function(d){return(d.rx_bytes*8/1e6).toFixed(2);});
  throughputChart=new Chart(canvas,{type:'line',data:{labels:labels,datasets:[
    {label:'TX Mbps',data:tx,borderColor:'#f78166',backgroundColor:'rgba(247,129,102,0.1)',borderWidth:2,fill:true,tension:0.3,pointRadius:1},
    {label:'RX Mbps',data:rx,borderColor:'#58a6ff',backgroundColor:'rgba(88,166,255,0.1)',borderWidth:2,fill:true,tension:0.3,pointRadius:1}
  ]},options:{responsive:true,maintainAspectRatio:false,interaction:{intersect:false,mode:'index'},
    scales:{x:{title:{display:true,text:'Time',color:'#8b949e'},ticks:{color:'#8b949e'},grid:{color:'#21262d'}},
      y:{title:{display:true,text:'Mbps',color:'#8b949e'},ticks:{color:'#8b949e'},grid:{color:'#21262d'},beginAtZero:true}},
    plugins:{legend:{labels:{color:'#e1e4e8'}},tooltip:{backgroundColor:'#161b22',borderColor:'#30363d',borderWidth:1,titleColor:'#e1e4e8',bodyColor:'#8b949e'}}}});
}
</script>
</body>
</html>"##,
    ext = "html"
)]
struct DashboardTemplate {
    ip: String,
}

// ---------------------------------------------------------------------------
// JSON response types
// ---------------------------------------------------------------------------

/// A single test session as returned by the sessions API.
#[derive(Serialize)]
struct SessionJson {
    id: i64,
    username: String,
    peer_ip: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    tx_bytes: i64,
    rx_bytes: i64,
    protocol: Option<String>,
    direction: Option<String>,
}

/// Aggregate statistics for an IP address.
#[derive(Serialize)]
struct StatsJson {
    total_sessions: i64,
    total_tx_bytes: i64,
    total_rx_bytes: i64,
    avg_tx_mbps: f64,
    avg_rx_mbps: f64,
}

/// One second of throughput data within a session.
#[derive(Serialize)]
struct IntervalJson {
    second: i64,
    tx_bytes: i64,
    rx_bytes: i64,
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

/// Uniform error wrapper so handlers can use `?` freely.
///
/// All errors are rendered as `500 Internal Server Error` with a plain-text
/// body. The full error chain is logged via [`tracing`].
struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!("web handler error: {:#}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /` -- render the landing page.
async fn index_page() -> Result<Html<String>, AppError> {
    let rendered = IndexTemplate
        .render()
        .map_err(|e| anyhow::anyhow!("template render: {}", e))?;
    Ok(Html(rendered))
}

/// `GET /dashboard/{ip}` -- render the per-IP dashboard.
async fn dashboard_page(Path(ip): Path<String>) -> Result<Html<String>, AppError> {
    let rendered = DashboardTemplate { ip }
        .render()
        .map_err(|e| anyhow::anyhow!("template render: {}", e))?;
    Ok(Html(rendered))
}

/// `GET /api/ip/{ip}/sessions` -- return the most recent 100 sessions for
/// the given peer IP as a JSON array.
async fn api_sessions(
    State(state): State<Arc<WebState>>,
    Path(ip): Path<String>,
) -> Result<axum::Json<Vec<SessionJson>>, AppError> {
    let sessions = {
        let conn = state
            .query_conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, username, peer_ip, started_at, ended_at,
                    tx_bytes, rx_bytes, protocol, direction
             FROM sessions
             WHERE peer_ip = ?1
             ORDER BY started_at DESC
             LIMIT 100",
        )?;
        let rows = stmt.query_map(params![ip], |row| {
            Ok(SessionJson {
                id: row.get(0)?,
                username: row.get(1)?,
                peer_ip: row.get(2)?,
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                tx_bytes: row.get(5)?,
                rx_bytes: row.get(6)?,
                protocol: row.get(7)?,
                direction: row.get(8)?,
            })
        })?;
        rows.filter_map(Result::ok).collect::<Vec<_>>()
    };

    Ok(axum::Json(sessions))
}

/// `GET /api/ip/{ip}/stats` -- return aggregate statistics (total bytes,
/// session count, average throughput) for the given IP.
async fn api_stats(
    State(state): State<Arc<WebState>>,
    Path(ip): Path<String>,
) -> Result<axum::Json<StatsJson>, AppError> {
    let stats = {
        let conn = state
            .query_conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {}", e))?;
        conn.query_row(
            "SELECT
                 COUNT(*)                                      AS total_sessions,
                 COALESCE(SUM(tx_bytes), 0)                    AS total_tx,
                 COALESCE(SUM(rx_bytes), 0)                    AS total_rx,
                 COALESCE(SUM(
                     CASE WHEN ended_at IS NOT NULL AND started_at IS NOT NULL
                          THEN (julianday(ended_at) - julianday(started_at)) * 86400.0
                          ELSE 0 END
                 ), 0)                                         AS total_seconds
             FROM sessions
             WHERE peer_ip = ?1",
            params![ip],
            |row| {
                let total_sessions: i64 = row.get(0)?;
                let total_tx: i64 = row.get(1)?;
                let total_rx: i64 = row.get(2)?;
                let total_seconds: f64 = row.get(3)?;

                let avg_tx_mbps = if total_seconds > 0.0 {
                    (total_tx as f64) * 8.0 / total_seconds / 1_000_000.0
                } else {
                    0.0
                };
                let avg_rx_mbps = if total_seconds > 0.0 {
                    (total_rx as f64) * 8.0 / total_seconds / 1_000_000.0
                } else {
                    0.0
                };

                Ok(StatsJson {
                    total_sessions,
                    total_tx_bytes: total_tx,
                    total_rx_bytes: total_rx,
                    avg_tx_mbps,
                    avg_rx_mbps,
                })
            },
        )?
    };

    Ok(axum::Json(stats))
}

/// `GET /api/session/{id}/intervals` -- return per-second throughput data
/// for a session.
///
/// If the `session_intervals` table does not exist or contains no rows for
/// the requested session, an empty JSON array is returned.
async fn api_intervals(
    State(state): State<Arc<WebState>>,
    Path(id): Path<i64>,
) -> Result<axum::Json<Vec<IntervalJson>>, AppError> {
    let intervals = {
        let conn = state
            .query_conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {}", e))?;

        // Guard against the table not existing (e.g. first run before
        // `ensure_web_tables` was ever called on this database file).
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'session_intervals'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            Vec::new()
        } else {
            let mut stmt = conn.prepare(
                "SELECT second, tx_bytes, rx_bytes
                 FROM session_intervals
                 WHERE session_id = ?1
                 ORDER BY second ASC",
            )?;
            let rows = stmt.query_map(params![id], |row| {
                Ok(IntervalJson {
                    second: row.get(0)?,
                    tx_bytes: row.get(1)?,
                    rx_bytes: row.get(2)?,
                })
            })?;
            rows.filter_map(Result::ok).collect::<Vec<_>>()
        }
    };

    Ok(axum::Json(intervals))
}
