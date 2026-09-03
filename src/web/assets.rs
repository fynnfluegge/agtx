//! The PWA, compiled into the binary.
//!
//! Embedded with `include_bytes!` rather than a bundler-plus-`rust-embed`
//! pipeline, for three reasons that all point the same way:
//!
//! - **There is nothing to bundle.** The app is plain ES modules with no npm
//!   dependency, which every browser that can install a PWA loads natively.
//! - **A missing file is a compile error.** A path table checked at build time
//!   cannot drift from what is on disk; a directory glob resolved at runtime
//!   404s instead, and only for whoever hits that asset.
//! - **No build artifact means nothing can be stale.** The alternative —
//!   committing `web/dist/` and adding a CI job to catch a stale copy — exists
//!   only to police a build step that this does not have.
//!
//! It also matches how `src/skills.rs` embeds the builtin skills, so there is
//! one answer in this codebase to "where do bundled files live".
//!
//! When Step 4 adds xterm.js, vendor it here as one more file rather than
//! introducing a toolchain to produce it.

/// One embedded file: request path, content type, bytes.
pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub body: &'static [u8],
}

/// Serving `index.html` at `/` is what makes a bare tunnel hostname work.
pub const INDEX: &str = "index.html";

pub const ASSETS: &[Asset] = &[
    Asset {
        path: "index.html",
        mime: "text/html; charset=utf-8",
        body: include_bytes!("../../web/index.html"),
    },
    Asset {
        path: "app.css",
        mime: "text/css; charset=utf-8",
        body: include_bytes!("../../web/app.css"),
    },
    // `text/javascript` and not `application/javascript`: a module script
    // served as the latter is fine, but a browser refuses any `type="module"`
    // whose MIME is not a JavaScript type, and this is the spelling the HTML
    // spec names. Getting it wrong fails the whole app with a console error
    // and a blank page.
    Asset {
        path: "app.js",
        mime: "text/javascript; charset=utf-8",
        body: include_bytes!("../../web/app.js"),
    },
    Asset {
        path: "api.js",
        mime: "text/javascript; charset=utf-8",
        body: include_bytes!("../../web/api.js"),
    },
    Asset {
        path: "sw.js",
        mime: "text/javascript; charset=utf-8",
        body: include_bytes!("../../web/sw.js"),
    },
    Asset {
        path: "manifest.webmanifest",
        mime: "application/manifest+json",
        body: include_bytes!("../../web/manifest.webmanifest"),
    },
    Asset {
        path: "icon-192.png",
        mime: "image/png",
        body: include_bytes!("../../web/icon-192.png"),
    },
    Asset {
        path: "icon-512.png",
        mime: "image/png",
        body: include_bytes!("../../web/icon-512.png"),
    },
];

/// Look up an asset by request path, with `""` and `index.html` both meaning
/// the shell.
pub fn find(path: &str) -> Option<&'static Asset> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { INDEX } else { path };
    ASSETS.iter().find(|a| a.path == path)
}
