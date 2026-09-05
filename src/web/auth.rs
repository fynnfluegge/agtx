//! Pairing, and the per-device tokens it issues.
//!
//! Without a credential the honest answer to "bind me to the network" is no:
//! anything that reaches this server can read every task diff, watch every
//! agent pane, and type into a running agent.
//!
//! Shape, and why each part is the way it is:
//!
//! - **One token per device**, hashed at rest in `mobile_devices`. A single
//!   shared secret cannot be taken back from one lost phone without cutting off
//!   every other device at the same time.
//! - **Pairing codes are short-lived, single-use and in memory only.** They
//!   never touch disk because they are only alive for the seconds between the
//!   QR appearing and a phone scanning it.
//! - **Static assets are open; `/api/*` and `/ws` are not.** A browser cannot
//!   put a header on the initial page load, so gating the shell would mean
//!   putting a credential in the URL of every navigation. The assets are the
//!   application's own JS and CSS and carry no board data.
//! - **Secrets travel in the URL fragment**, which browsers do not transmit, so
//!   they stay out of proxy and server logs. The page moves them to local
//!   storage and replaces the hash.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Where the token lives. Beside `config.toml` rather than in the platform data
/// dir — see the config-path split in CLAUDE.md — and honouring
/// `AGTX_CONFIG_DIR` so tests never touch the real one.
pub fn token_path() -> Result<PathBuf> {
    let cfg = crate::config::GlobalConfig::config_path()?;
    Ok(cfg
        .parent()
        .context("config path has no parent")?
        .join("serve-token"))
}

/// Load the legacy single token, creating one on first use.
///
/// Superseded by per-device pairing; kept only so an already-paired phone
/// survives the upgrade — see [`migrate_legacy_token`].
pub fn load_or_create() -> Result<String> {
    let path = token_path()?;
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let token = generate();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, &token).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// 192 bits, hex, from the OS.
///
/// `getrandom` via `uuid`'s v4 generator rather than a hand-rolled PRNG: agtx
/// already depends on it, and a token seeded from the clock is the classic way
/// to make a credential guessable.
fn generate() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{}", &b[..16])
}

/// Whether `presented` matches `expected`, compared in constant time.
///
/// A byte-by-byte `==` returns early on the first difference, which leaks the
/// length of the matching prefix to anyone who can time the response. That is a
/// thin channel over a network but a real one, and the fix costs nothing.
pub fn token_matches(expected: &str, presented: &str) -> bool {
    let (a, b) = (expected.as_bytes(), presented.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The `Sec-WebSocket-Protocol` value carrying a token.
///
/// A browser cannot set `Authorization` on a WebSocket handshake, and a query
/// parameter would put the token in exactly the logs the fragment avoids. The
/// subprotocol header is the one client-settable field on that handshake.
pub const WS_TOKEN_PREFIX: &str = "agtx.token.";

// ── pairing ─────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::db::{Database, MobileDevice};

/// How long a pairing code is good for.
///
/// Short because the code's whole life is the seconds between a QR appearing
/// in a terminal and a phone scanning it. A code that outlives the person
/// standing in front of the screen is a credential nobody is watching.
pub const PAIRING_TTL: Duration = Duration::from_secs(120);

/// Failed `/api/pair` attempts before pairing is shut until the next code.
///
/// A code is 128 bits, so guessing is hopeless on arithmetic alone — but
/// `/api/pair` is the one unauthenticated route, and letting it be hammered
/// freely is how a rate-limit oversight becomes the interesting part of an
/// incident report.
pub const PAIRING_MAX_FAILURES: u32 = 10;

/// Codes waiting to be exchanged. In memory only, deliberately.
#[derive(Default)]
pub struct PairingCodes {
    codes: Mutex<HashMap<String, Instant>>,
    failures: Mutex<u32>,
}

/// Why a pairing attempt was refused. Distinct cases because the fix differs:
/// a stale code means scan again, a locked-out server means restart it.
#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    Unknown,
    Expired,
    LockedOut,
}

impl PairError {
    pub fn message(&self) -> &'static str {
        match self {
            PairError::Unknown => "that pairing code is not valid; scan the current QR code",
            PairError::Expired => "that pairing code has expired; restart agtx serve for a new one",
            PairError::LockedOut => {
                "too many failed pairing attempts; restart agtx serve to pair again"
            }
        }
    }
}

impl PairingCodes {
    /// Adopt a code minted elsewhere.
    ///
    /// The TUI generates the code so it can build the pairing URL — and draw
    /// its QR — before the child server has started. The alternative is parsing
    /// the URL back out of the child's stdout, which makes a human-readable
    /// banner load-bearing and breaks quietly when its wording changes.
    pub fn seed(&self, code: &str) {
        let mut codes = self.codes.lock().unwrap_or_else(|e| e.into_inner());
        codes.insert(code.to_string(), Instant::now());
    }

    /// Mint a code and remember it until it is used or expires.
    pub fn issue(&self) -> String {
        let code = uuid::Uuid::new_v4().simple().to_string();
        let mut codes = self.codes.lock().unwrap_or_else(|e| e.into_inner());
        codes.retain(|_, issued| issued.elapsed() < PAIRING_TTL);
        codes.insert(code.clone(), Instant::now());
        code
    }

    /// Spend a code. Removed whether or not it had expired: a code presented
    /// once is finished either way, and leaving an expired one in the map lets
    /// it be retried until the sweep.
    pub fn redeem(&self, code: &str) -> Result<(), PairError> {
        {
            let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
            if *failures >= PAIRING_MAX_FAILURES {
                return Err(PairError::LockedOut);
            }
        }

        let mut codes = self.codes.lock().unwrap_or_else(|e| e.into_inner());
        match codes.remove(code) {
            Some(issued) if issued.elapsed() < PAIRING_TTL => {
                *self.failures.lock().unwrap_or_else(|e| e.into_inner()) = 0;
                Ok(())
            }
            Some(_) => Err(PairError::Expired),
            None => {
                *self.failures.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                Err(PairError::Unknown)
            }
        }
    }
}

/// Mint a device token, store its hash, and hand the token back.
///
/// The token is returned exactly once and never stored — only its hash — so a
/// device that loses it has to pair again rather than ask for it back.
///
/// `session_id` records which serve session paired it. The pairing itself
/// persists until revoked.
pub fn pair_device(label: &str, session_id: Option<&str>) -> Result<String> {
    let token = generate();
    let mut device = MobileDevice::new(sanitise_label(label), sha(&token));
    device.session_id = session_id.map(str::to_string);
    Database::open_global()
        .context("opening the global database")?
        .add_mobile_device(&device)
        .context("storing the paired device")?;
    Ok(token)
}

/// The device this token belongs to, if any.
pub fn device_for_token(token: &str) -> Option<MobileDevice> {
    Database::open_global()
        .ok()?
        .mobile_device_by_hash(&sha(token))
        .ok()
        .flatten()
}

pub fn sha(token: &str) -> String {
    crate::update::install::sha256_hex(token.as_bytes())
}

/// A label is shown in a device list and nowhere else, so the only rules are
/// that it exists and cannot flood the display.
fn sanitise_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() {
        "phone".to_string()
    } else {
        cleaned
    }
}

/// Adopt an existing `serve-token` as a paired device, once.
///
/// Without this the move to per-device tokens would silently stop an
/// already-paired phone from connecting, with nothing on screen to explain why.
/// The file is removed afterwards so there is exactly one credential path.
///
/// The adopted device records `session_id` like any other.
pub fn migrate_legacy_token(session_id: Option<&str>) -> Result<Option<String>> {
    let path = token_path()?;
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let token = existing.trim().to_string();
    if token.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }

    let db = Database::open_global().context("opening the global database")?;
    if db.mobile_device_by_hash(&sha(&token))?.is_none() {
        let mut device = MobileDevice::new("previously paired device", sha(&token));
        device.session_id = session_id.map(str::to_string);
        db.add_mobile_device(&device)?;
    }
    let _ = std::fs::remove_file(&path);
    Ok(Some(token))
}
