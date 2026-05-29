//! Daemon connect + probe (ROB-01, PITFALLS Pitfall 9).
//!
//! This module exposes one thing to the rest of the app:
//! [`connect_and_probe`] — connect to the local Docker daemon via bollard's
//! local-defaults (unix socket on Linux/macOS, named pipe on Windows) and
//! confirm the connection actually works by issuing a lightweight
//! `version` request.
//!
//! Why probe at all (and BEFORE the TUI):
//!
//! PITFALLS Pitfall 9 — "Daemon down / socket permission errors handled as a
//! crash". If the daemon is down, or the user isn't in the `docker` group, or
//! the socket simply isn't there, naive `unwrap()` panics produce a garbled
//! terminal (raw mode + alt screen + panic) and zero useful information. The
//! probe is the cheap, deterministic check that lets `main.rs` (wired in 03-04)
//! handle the failure with a CLEAN, plain-text, actionable message on stderr
//! BEFORE [`crate::tui::Tui::enter`] flips the terminal into raw mode.
//!
//! Classification policy:
//!
//! - **SocketMissing** — `/var/run/docker.sock` doesn't exist. Most often: no
//!   Docker installed, or Docker Desktop / colima isn't running, or rootless
//!   socket lives at a different path.
//! - **PermissionDenied** — socket exists but the current user can't open it.
//!   Classic "add yourself to the `docker` group" case (or use the rootless
//!   socket if that's the install).
//! - **DaemonDown** — socket exists and we can open it, but the daemon
//!   doesn't respond (connection refused / reset / closed). Most often: the
//!   service is stopped while the socket file lingers.
//! - **Other** — anything we can't reliably classify; we surface the raw
//!   bollard error so the user has SOME signal instead of a generic "failed".
//!
//! Classification is best-effort: bollard 0.21 funnels most low-level errors
//! through `hyper` / `hyper_util` legacy clients, so we walk the
//! `std::error::Error::source()` chain looking for a `std::io::Error` whose
//! `kind()` is informative. When in doubt, we return `DaemonDown` with the
//! raw error text appended so the user has SOMETHING actionable instead of an
//! opaque panic.
//!
//! Out of scope here:
//!
//! - No terminal I/O. This function never touches `stdout`/`stderr` — the
//!   caller (main.rs in 03-04) prints the error message and exits. That
//!   keeps this function testable and reusable from both backends (kitty +
//!   braille).
//! - No stream subscription. `events()` / `stats(id)` streams live in
//!   `docker/streams.rs` (03-03). The probe is `version()` only — one round
//!   trip, no stream lifetime to manage.

#![allow(dead_code)]

use std::error::Error as StdError;
use std::fmt;
use std::io;

use bollard::Docker;

/// What we tell the user when the daemon isn't usable. Each variant's
/// [`fmt::Display`] message is the EXACT plain-text string the caller prints
/// to stderr before exiting — keep them short, actionable, and self-contained
/// (the user may see only this message and nothing else).
#[derive(Debug)]
pub enum ProbeError {
    /// The Docker socket file doesn't exist at the expected path.
    ///
    /// Default socket path on this platform — comes from bollard
    /// (`/var/run/docker.sock` on Linux/macOS, `\\.\pipe\docker_engine` on
    /// Windows) or `DOCKER_HOST` if it's set.
    SocketMissing { path: String },

    /// The socket exists but the current user can't open it (EACCES).
    /// Classic "user isn't in the `docker` group" case.
    PermissionDenied { path: String },

    /// The socket can be opened, but the daemon isn't responding to the probe.
    /// `detail` is the raw underlying error text so the user has something to
    /// search for if the hint isn't enough.
    DaemonDown { path: String, detail: String },

    /// Anything we can't reliably classify — surface the raw bollard error
    /// instead of swallowing the signal.
    Other(String),
}

impl ProbeError {
    /// The exact plain-text message [`connect_and_probe`]'s caller should
    /// print to stderr. Equivalent to formatting via [`fmt::Display`]; spelled
    /// out as a method so call sites can grep for it.
    pub fn user_message(&self) -> String {
        format!("{self}")
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::SocketMissing { path } => write!(
                f,
                "Docker socket not found at {path} — is Docker installed and running?"
            ),
            ProbeError::PermissionDenied { path } => write!(
                f,
                "Permission denied on {path} — add your user to the `docker` group or use the rootless socket."
            ),
            ProbeError::DaemonDown { path, detail } => write!(
                f,
                "Docker daemon not reachable at {path} — is it running? (try: systemctl start docker) [{detail}]"
            ),
            ProbeError::Other(detail) => write!(
                f,
                "Could not talk to the Docker daemon: {detail}"
            ),
        }
    }
}

impl StdError for ProbeError {}

/// Connect to the local Docker daemon and probe it.
///
/// Returns a live [`Docker`] handle on success — that handle is the one the
/// rest of the app (03-03 streams) reuses; no need to reconnect.
///
/// On failure, returns a classified [`ProbeError`] with a self-contained
/// plain-text message. The CALLER prints it to stderr and exits — this
/// function performs NO terminal I/O of its own (must remain safe to call
/// before the TUI enters raw mode; see ROB-01).
///
/// The probe is a single `version()` request. We don't use `ping()` because
/// some bollard versions return `Result<String, Error>` and a "PONG" string
/// isn't more useful than the `SystemVersion` struct for our purposes (and
/// `version()` exercises the full request/parse path).
pub async fn connect_and_probe() -> Result<Docker, ProbeError> {
    // Resolve the path the user will see in error messages. bollard's
    // `connect_with_local_defaults` reads DOCKER_HOST if set, otherwise uses
    // `/var/run/docker.sock` (unix) / `\\.\pipe\docker_engine` (windows). We
    // mirror that logic ONLY for the error message — bollard does the actual
    // connection.
    let path = effective_socket_path();

    // Step 1: build the client. This is sync (no I/O on Unix beyond a
    // `Path::exists()` check inside bollard) but it CAN fail with
    // SocketNotFoundError if the path doesn't exist — classify that first.
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(err) => return Err(classify(&err, &path)),
    };

    // Step 2: probe with a real round-trip. Until we await this, we haven't
    // actually opened the socket — so this is where DaemonDown /
    // PermissionDenied surface.
    match docker.version().await {
        Ok(_) => Ok(docker),
        Err(err) => Err(classify(&err, &path)),
    }
}

/// Classify a bollard error using the platform-default socket path for the
/// user-facing message. Convenience wrapper around [`classify`] for callers
/// that don't keep their own resolved path (the off-thread `inspect` path
/// in 04-06a calls this on every bollard error from `fetch_detail`).
pub(crate) fn classify_default_path(err: &bollard::errors::Error) -> ProbeError {
    let path = effective_socket_path();
    classify(err, &path)
}

/// Resolve the socket path we'll mention in error messages. Honors
/// `DOCKER_HOST` (the bollard default) when set, otherwise falls back to the
/// platform default.
fn effective_socket_path() -> String {
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if !host.is_empty() {
            return host;
        }
    }
    #[cfg(unix)]
    {
        "/var/run/docker.sock".to_string()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\docker_engine".to_string()
    }
}

/// Classify a bollard [`bollard::errors::Error`] into a [`ProbeError`].
///
/// Best-effort: bollard wraps lots of underlying errors (hyper, hyper-util
/// legacy, io). We special-case the obviously-classifiable variants and walk
/// the `source()` chain looking for an `io::Error` with an informative
/// `kind()`. Anything we can't classify lands in [`ProbeError::DaemonDown`]
/// (the most common real-world cause when classification fails) with the raw
/// error appended — or [`ProbeError::Other`] when we genuinely have no signal.
///
/// `pub(crate)` so [`crate::docker::inspect::fetch_detail`] (04-06a) reuses
/// the same 4-variant error classifier instead of inventing its own — every
/// inspect-style call routes user-facing errors through one place. The
/// `effective_socket_path()` helper is invoked by callers via
/// [`classify_default_path`] so the path string the user sees is consistent.
pub(crate) fn classify(err: &bollard::errors::Error, path: &str) -> ProbeError {
    use bollard::errors::Error as B;

    // Easy case: bollard already pre-classified this as "socket file missing".
    if let B::SocketNotFoundError(p) = err {
        return ProbeError::SocketMissing {
            path: if p.is_empty() { path.to_string() } else { p.clone() },
        };
    }

    // Easy case: a direct IO error wrapped by bollard.
    if let B::IOError { err: io_err } = err {
        return io_to_probe(io_err, path);
    }

    // Harder case: hyper / hyper-util / generic errors. Walk the source chain
    // looking for an io::Error we can read a kind() off of.
    if let Some(io_err) = find_io_in_chain(err) {
        return io_to_probe(io_err, path);
    }

    // Unclassified: most often the daemon is up but mid-handshake something
    // failed — surface as DaemonDown with the raw text so the user has SOME
    // hint. If the message looks unrelated, the `Other` variant exists for
    // truly weird stuff (parse errors, etc.); we use it only when the bollard
    // variant itself indicates a non-connection failure.
    match err {
        B::JsonDataError { .. } | B::JsonSerdeError { .. } | B::APIVersionParseError { .. } => {
            ProbeError::Other(err.to_string())
        }
        _ => ProbeError::DaemonDown {
            path: path.to_string(),
            detail: err.to_string(),
        },
    }
}

/// Map a `std::io::Error` (the deepest signal we usually have) to a
/// [`ProbeError`] using the path the caller expects to see in messages.
fn io_to_probe(err: &io::Error, path: &str) -> ProbeError {
    match err.kind() {
        io::ErrorKind::PermissionDenied => ProbeError::PermissionDenied {
            path: path.to_string(),
        },
        io::ErrorKind::NotFound => ProbeError::SocketMissing {
            path: path.to_string(),
        },
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::NotConnected
        | io::ErrorKind::TimedOut => ProbeError::DaemonDown {
            path: path.to_string(),
            detail: err.to_string(),
        },
        _ => ProbeError::DaemonDown {
            path: path.to_string(),
            detail: err.to_string(),
        },
    }
}

/// Walk the `Error::source()` chain and return the first `io::Error` we see.
/// `dyn Error::source()` is the standard way to dig through wrapped errors
/// without depending on `anyhow` / `eyre` here.
fn find_io_in_chain<'a>(err: &'a (dyn StdError + 'static)) -> Option<&'a io::Error> {
    let mut current: Option<&(dyn StdError + 'static)> = err.source();
    while let Some(e) = current {
        if let Some(io_err) = e.downcast_ref::<io::Error>() {
            return Some(io_err);
        }
        current = e.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Display messages: each variant carries its actionable hint ----------

    #[test]
    fn socket_missing_message_mentions_path_and_install_hint() {
        let m = ProbeError::SocketMissing {
            path: "/var/run/docker.sock".to_string(),
        }
        .to_string();
        assert!(m.contains("/var/run/docker.sock"), "msg: {m}");
        assert!(
            m.to_lowercase().contains("docker")
                && (m.contains("installed") || m.contains("running")),
            "msg should hint about install/run state: {m}"
        );
    }

    #[test]
    fn permission_denied_message_mentions_docker_group() {
        let m = ProbeError::PermissionDenied {
            path: "/var/run/docker.sock".to_string(),
        }
        .to_string();
        assert!(m.contains("/var/run/docker.sock"), "msg: {m}");
        assert!(
            m.contains("docker group") || m.contains("`docker` group"),
            "permission_denied should mention the `docker` group fix: {m}"
        );
    }

    #[test]
    fn daemon_down_message_mentions_running_and_detail() {
        let m = ProbeError::DaemonDown {
            path: "/var/run/docker.sock".to_string(),
            detail: "connection refused".to_string(),
        }
        .to_string();
        assert!(m.contains("/var/run/docker.sock"), "msg: {m}");
        assert!(
            m.to_lowercase().contains("running")
                || m.to_lowercase().contains("daemon"),
            "daemon_down should hint at starting the daemon: {m}"
        );
        assert!(
            m.contains("connection refused"),
            "daemon_down should include the raw detail: {m}"
        );
    }

    #[test]
    fn other_message_includes_raw_detail() {
        let m = ProbeError::Other("weird".to_string()).to_string();
        assert!(m.contains("weird"), "Other should surface the raw text: {m}");
    }

    #[test]
    fn user_message_equals_display() {
        let e = ProbeError::PermissionDenied {
            path: "/sock".to_string(),
        };
        assert_eq!(e.user_message(), e.to_string());
    }

    // --- Classifier: maps io::ErrorKind -> the right ProbeError --------------

    #[test]
    fn io_permission_denied_classifies_as_permission_denied() {
        let io_err = io::Error::from(io::ErrorKind::PermissionDenied);
        let p = io_to_probe(&io_err, "/var/run/docker.sock");
        assert!(matches!(p, ProbeError::PermissionDenied { .. }));
    }

    #[test]
    fn io_not_found_classifies_as_socket_missing() {
        let io_err = io::Error::from(io::ErrorKind::NotFound);
        let p = io_to_probe(&io_err, "/var/run/docker.sock");
        assert!(matches!(p, ProbeError::SocketMissing { .. }));
    }

    #[test]
    fn io_connection_refused_classifies_as_daemon_down() {
        let io_err = io::Error::from(io::ErrorKind::ConnectionRefused);
        let p = io_to_probe(&io_err, "/var/run/docker.sock");
        assert!(matches!(p, ProbeError::DaemonDown { .. }));
    }

    #[test]
    fn unknown_io_kind_still_falls_back_to_daemon_down() {
        // Unknown / unexpected io kinds don't get swallowed — they become
        // DaemonDown with the raw text in `detail` so the user sees SOMETHING.
        let io_err = io::Error::other("weird underlying io failure");
        let p = io_to_probe(&io_err, "/var/run/docker.sock");
        match p {
            ProbeError::DaemonDown { detail, .. } => {
                assert!(detail.contains("weird"), "raw detail should be surfaced");
            }
            _ => panic!("expected DaemonDown fallback"),
        }
    }

    // --- bollard error classification ----------------------------------------

    #[test]
    fn bollard_socket_not_found_classifies_as_socket_missing() {
        let err = bollard::errors::Error::SocketNotFoundError("/tmp/nope.sock".to_string());
        let p = classify(&err, "/var/run/docker.sock");
        match p {
            ProbeError::SocketMissing { path } => {
                // bollard's own path beats our default when it's non-empty.
                assert_eq!(path, "/tmp/nope.sock");
            }
            _ => panic!("expected SocketMissing"),
        }
    }

    #[test]
    fn bollard_io_permission_classifies_as_permission_denied() {
        let err = bollard::errors::Error::IOError {
            err: io::Error::from(io::ErrorKind::PermissionDenied),
        };
        let p = classify(&err, "/var/run/docker.sock");
        assert!(matches!(p, ProbeError::PermissionDenied { .. }));
    }

    #[test]
    fn bollard_io_connection_refused_classifies_as_daemon_down() {
        let err = bollard::errors::Error::IOError {
            err: io::Error::from(io::ErrorKind::ConnectionRefused),
        };
        let p = classify(&err, "/var/run/docker.sock");
        assert!(matches!(p, ProbeError::DaemonDown { .. }));
    }

    // --- ROB-01 invariant: classify never panics, always yields a message ----

    #[test]
    fn every_classification_produces_a_nonempty_message() {
        let cases = [
            ProbeError::SocketMissing {
                path: "/s".to_string(),
            },
            ProbeError::PermissionDenied {
                path: "/s".to_string(),
            },
            ProbeError::DaemonDown {
                path: "/s".to_string(),
                detail: "x".to_string(),
            },
            ProbeError::Other("x".to_string()),
        ];
        for c in cases {
            let m = c.to_string();
            assert!(!m.is_empty(), "every variant must produce a message");
            assert!(m.len() > 10, "messages should be actionable, not stubs: {m}");
        }
    }

    // --- effective_socket_path: respects DOCKER_HOST ------------------------

    #[test]
    fn effective_socket_path_falls_back_to_platform_default() {
        // We can't safely set/unset env in a parallel test runner without
        // races. Instead, assert the unix default path appears when
        // DOCKER_HOST is empty/unset — done by reading the current env at
        // most once; if DOCKER_HOST is set in the test env we accept that
        // value (any non-empty string is fine).
        let p = effective_socket_path();
        assert!(!p.is_empty(), "path must never be empty");
        #[cfg(unix)]
        {
            if std::env::var("DOCKER_HOST").ok().filter(|s| !s.is_empty()).is_none() {
                assert_eq!(p, "/var/run/docker.sock");
            }
        }
    }
}
