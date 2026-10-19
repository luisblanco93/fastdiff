#!/usr/bin/env python3
"""Build the fake `meridian` repository used for the README screenshots.

usage: make-demo-repo.py <repo-dir> <bare-remote-dir>
"""
import os
import struct
import subprocess
import sys
import zlib
from pathlib import Path

repo = Path(sys.argv[1])
remote = Path(sys.argv[2])
clock = [1760000000]


def git(*args, **extra):
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "Maya Chen",
        "GIT_AUTHOR_EMAIL": "maya@meridian.example",
        "GIT_COMMITTER_NAME": "Maya Chen",
        "GIT_COMMITTER_EMAIL": "maya@meridian.example",
        "GIT_AUTHOR_DATE": f"{clock[0]} +0000",
        "GIT_COMMITTER_DATE": f"{clock[0]} +0000",
        **extra,
    }
    subprocess.run(["git", *args], cwd=repo, env=env, check=True, stdout=subprocess.DEVNULL)


def write(path, text):
    target = repo / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def commit(message):
    clock[0] += 3 * 3600
    git("add", "-A")
    git("commit", "-q", "-m", message)


def png(color, ring):
    size = 96
    rows = bytearray()
    for y in range(size):
        rows.append(0)
        for x in range(size):
            distance = ((x - size / 2) ** 2 + (y - size / 2) ** 2) ** 0.5
            rows += bytes(ring if 22 < distance < 34 else color)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(rows))) + chunk(b"IEND", b"")


MAIN_HEAD = """mod config;
mod routes;

use std::net::SocketAddr;

use axum::{routing::get, Router};
use config::Config;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let cfg = Config::from_env();
    let app = Router::new()
        .route("/orders", get(routes::orders::list))
        .route("/users", get(routes::users::list));

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    tracing::info!("listening on {addr}");
    let listener = TcpListener::bind(addr)
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
"""
MAIN_STAGED = MAIN_HEAD.replace(
    '.route("/users", get(routes::users::list));',
    '.route("/accounts", get(routes::accounts::list));',
)
MAIN_WORKING = MAIN_STAGED.replace(
    "    let app = Router::new()\n",
    '    let app = Router::new()\n        .route("/health", get(routes::health::check))\n',
).replace(
    'tracing::info!("listening on {addr}");',
    'tracing::info!(%addr, "meridian ready");',
)

CONFIG_BASE = """use std::env;

pub struct Config {
    pub port: u16,
    pub database_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            port: env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
        }
    }
}
"""
CONFIG_UNPUSHED = CONFIG_BASE.replace(
    "    pub database_url: String,\n",
    "    pub database_url: String,\n    pub request_timeout_secs: u64,\n",
).replace(
    '            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),\n',
    '            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),\n'
    '            request_timeout_secs: env::var("REQUEST_TIMEOUT_SECS")\n'
    '                .ok()\n'
    '                .and_then(|s| s.parse().ok())\n'
    '                .unwrap_or(30),\n',
)

ORDERS_HEAD = """use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct LineItem {
    pub sku: String,
    pub quantity: u32,
    pub unit_cents: u64,
}

#[derive(Serialize)]
pub struct Order {
    pub id: u64,
    pub customer: String,
    pub items: Vec<LineItem>,
}

impl Order {
    pub fn total_cents(&self) -> u64 {
        self.items
            .iter()
            .map(|item| item.unit_cents * item.quantity as u64)
            .sum()
    }
}

pub async fn list() -> Json<Vec<Order>> {
    Json(sample_orders())
}

/// Fixture data until the orders table lands.
fn sample_orders() -> Vec<Order> {
    vec![
        Order {
            id: 1001,
            customer: "Northwind Traders".into(),
            items: vec![
                LineItem { sku: "MR-100".into(), quantity: 4, unit_cents: 1250 },
                LineItem { sku: "MR-220".into(), quantity: 1, unit_cents: 8900 },
            ],
        },
        Order {
            id: 1002,
            customer: "Alder & Finch".into(),
            items: vec![LineItem { sku: "MR-100".into(), quantity: 12, unit_cents: 1250 }],
        },
    ]
}
"""
ORDERS_UNPUSHED = ORDERS_HEAD.replace(
    "impl Order {\n",
    """const MAX_QUANTITY: u32 = 10_000;

#[derive(Debug)]
pub enum OrderError {
    EmptyOrder,
    QuantityOutOfRange { sku: String, quantity: u32 },
}

impl Order {
    pub fn validate(&self) -> Result<(), OrderError> {
        if self.items.is_empty() {
            return Err(OrderError::EmptyOrder);
        }
        for item in &self.items {
            if item.quantity == 0 || item.quantity > MAX_QUANTITY {
                return Err(OrderError::QuantityOutOfRange {
                    sku: item.sku.clone(),
                    quantity: item.quantity,
                });
            }
        }
        Ok(())
    }

""",
)
ORDERS_WORKING = ORDERS_UNPUSHED.replace(
    "    Json(sample_orders())\n",
    "    let mut orders = sample_orders();\n    orders.sort_by_key(|order| order.id);\n    Json(orders)\n",
)

CARGO_HEAD = """[package]
name = "meridian"
version = "0.3.1"
edition = "2021"

[dependencies]
axum = "0.7"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net"] }
tracing = "0.1"
tracing-subscriber = "0.3"
"""
CARGO_WORKING = CARGO_HEAD.replace('version = "0.3.1"', 'version = "0.4.0-dev"').replace(
    'tracing-subscriber = "0.3"\n',
    'tracing-subscriber = "0.3"\nuuid = { version = "1", features = ["v4"] }\n',
)

README_HEAD = """# meridian

Order service for the Meridian storefront. Serves the orders and accounts APIs
over HTTP and reads its settings from the environment.
"""
README_STAGED = README_HEAD + """
## Running

```
DATABASE_URL=postgres://localhost/meridian cargo run
```

The service listens on `PORT` (default 8080).
"""

API_BASE = """# Meridian API

## GET /orders

Returns every order with its line items.

## GET /users

Returns the registered users.
"""
API_UNPUSHED = API_BASE + """
## Validation errors

Orders are validated before they are stored:

| Error | Meaning |
| --- | --- |
| `EmptyOrder` | The order has no line items. |
| `QuantityOutOfRange` | A line item has a quantity of 0 or more than 10,000. |
"""

USERS_RS = """use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct User {
    pub id: u64,
    pub email: String,
}

pub async fn list() -> Json<Vec<User>> {
    Json(Vec::new())
}
"""
ACCOUNTS_RS = USERS_RS.replace("User", "Account")

LEGACY_HEAD = """//! Pre-1.0 order import helpers kept for the nightly batch job.

pub fn parse_legacy_id(raw: &str) -> Option<u64> {
    raw.trim_start_matches("ORD-").parse().ok()
}

pub fn format_legacy_id(id: u64) -> String {
    format!("ORD-{id:06}")
}
"""
LEGACY_STAGED = LEGACY_HEAD.replace(
    "pub fn parse_legacy_id",
    '#[deprecated(note = "use the v2 importer")]\npub fn parse_legacy_id',
).replace(
    "pub fn format_legacy_id",
    '#[deprecated(note = "use the v2 importer")]\npub fn format_legacy_id',
)

HEALTH_RS = """use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
}

pub async fn check() -> Json<Health> {
    Json(Health { status: "ok" })
}
"""
TODO = """- paginate GET /orders
- move sample_orders behind a repository trait
- decide what happens to legacy.rs after the v2 importer ships
"""

assert ORDERS_HEAD.count("\n") == 48, ORDERS_HEAD.count("\n")
assert ORDERS_UNPUSHED.count("\n") == 71, ORDERS_UNPUSHED.count("\n")
assert MAIN_HEAD.count("\n") == 23 and MAIN_WORKING.count("\n") == 24

repo.mkdir(parents=True, exist_ok=True)
remote.mkdir(parents=True, exist_ok=True)
subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(remote)], check=True)
git("init", "-q", "-b", "main")
git("config", "user.name", "Maya Chen")
git("config", "user.email", "maya@meridian.example")
git("remote", "add", "origin", str(remote))

write(".gitignore", "target/\n")
write("Cargo.toml", CARGO_HEAD)
write("README.md", README_HEAD)
write("src/main.rs", MAIN_HEAD.replace("mod routes;\n", "mod routes;\n", 1))
write("src/config.rs", CONFIG_BASE)
commit("chore: scaffold the meridian service")

write("src/routes/mod.rs", "pub mod orders;\npub mod users;\n")
write("src/routes/orders.rs", ORDERS_HEAD)
write("src/routes/users.rs", USERS_RS)
commit("feat: serve orders and users over HTTP")

write("src/legacy.rs", LEGACY_HEAD)
(repo / "assets").mkdir(exist_ok=True)
(repo / "assets/logo.png").write_bytes(png((18, 70, 84), (240, 200, 90)))
commit("chore: import the legacy id helpers and brand assets")

write("docs/api.md", API_BASE)
commit("docs: describe the orders and users endpoints")
git("push", "-q", "-u", "origin", "main")

git("switch", "-q", "-c", "feature/order-validation")
write("src/routes/orders.rs", ORDERS_HEAD.replace("fn sample_orders", "pub fn sample_orders"))
commit("refactor: expose the sample orders to tests")
git("push", "-q", "-u", "origin", "feature/order-validation")
git("switch", "-q", "main")
git("switch", "-q", "feature/order-validation")

write("src/routes/orders.rs", ORDERS_UNPUSHED.replace("fn sample_orders", "pub fn sample_orders"))
write("src/config.rs", CONFIG_UNPUSHED)
commit("feat: reject empty orders and out-of-range quantities")
write("docs/api.md", API_UNPUSHED)
commit("docs: list the order validation errors")

# Staged: rename users -> accounts, route wiring, deprecations, README.
write("src/main.rs", MAIN_STAGED)
write("src/routes/mod.rs", "pub mod accounts;\npub mod health;\npub mod orders;\n")
write("src/legacy.rs", LEGACY_STAGED)
write("README.md", README_STAGED)
git("mv", "src/routes/users.rs", "src/routes/accounts.rs")
write("src/routes/accounts.rs", ACCOUNTS_RS)
git("add", "-A")

# Unstaged edits on top, plus untracked files.
write("src/main.rs", MAIN_WORKING)
write("src/routes/orders.rs", ORDERS_WORKING.replace("fn sample_orders", "pub fn sample_orders"))
write("Cargo.toml", CARGO_WORKING)
(repo / "assets/logo.png").write_bytes(png((22, 84, 100), (250, 214, 110)))
write("src/routes/health.rs", HEALTH_RS)
write("TODO.txt", TODO)
