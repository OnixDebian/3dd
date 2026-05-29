//! Detail-panel popup (CAM-05 / 04-06b) — ratatui Clear + Block + Paragraph.
//!
//! Rendered OVER the 3D scene by `ui::view` when `selection.detail_open` is
//! true. Two states:
//!
//! - [`render_detail_panel`] — full popup with name, status, health, image,
//!   started, restart count + policy, network mode, ports list, mounts list,
//!   live block I/O totals, and a help footer. Called when
//!   `selection.pending_detail` is `Some`.
//! - [`render_loading`] — small "loading…" placeholder. Called when the user
//!   pressed Enter but the off-thread `fetch_detail` hasn't returned yet
//!   (`pending_detail` is `None` while `inspect_in_flight` is true).
//!
//! The popup is sized at 60% × 60% of the frame, centered. `Clear` blanks the
//! cells underneath so the underlying braille scene doesn't bleed through.
//! Truncation: mounts beyond the first 3 fold to "…N more" so a container
//! with 30+ binds doesn't overflow the popup height (Pitfall: panel must fit
//! on small terminals).
//!
//! Live block I/O is read from `LiveWorld::last_sample` EVERY frame at the
//! call site — the popup reflects the freshest blkio counters even while
//! open. `DetailSnapshot` (static container metadata) is cached on Enter;
//! `StatSample` (live counters) is sampled at ~1Hz per running container.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::docker::{DetailSnapshot, HealthSummary, MountSummary, PortSummary};

/// Maximum mounts / ports rendered inline before the "…N more" fold. Keeps
/// the popup height predictable on small terminals.
const MAX_INLINE_LIST: usize = 3;

/// Render the popup over the current frame.
///
/// `snap` is the cached metadata from the last `fetch_detail` call; `blkio_r`
/// and `blkio_w` are the freshest cumulative read/write bytes from
/// `LiveWorld::last_sample` (caller refreshes these every frame; popup is live
/// even while open).
pub fn render_detail_panel(
    frame: &mut Frame,
    snap: &DetailSnapshot,
    blkio_r: u64,
    blkio_w: u64,
) {
    let area = centered_rect(60, 60, frame.area());
    frame.render_widget(Clear, area);

    let title = Span::styled(
        format!(" {} ", snap.name),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let block = Block::default().title(title).borders(Borders::ALL);

    let mut lines: Vec<Line> = Vec::with_capacity(20);
    lines.push(Line::from(format!("Status:     {:?}", snap.status)));
    lines.push(Line::from(format!("Health:     {}", health_str(&snap.health))));
    lines.push(Line::from(format!("Image:      {}", snap.image_human)));
    lines.push(Line::from(format!("Started:    {}", snap.started_at_iso)));
    lines.push(Line::from(format!(
        "Restarts:   {} ({})",
        snap.restart_count, snap.restart_policy
    )));
    lines.push(Line::from(format!("Network:    {}", snap.network_mode)));
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Ports ({}):", snap.ports.len())));
    lines.push(Line::from(format!("  {}", format_ports(&snap.ports))));
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Mounts ({}):", snap.mounts.len())));
    for m in snap.mounts.iter().take(MAX_INLINE_LIST) {
        lines.push(Line::from(format!(
            "  {} ({})",
            format_mount(m),
            if m.rw { "rw" } else { "ro" }
        )));
    }
    if snap.mounts.len() > MAX_INLINE_LIST {
        lines.push(Line::from(format!(
            "  …{} more",
            snap.mounts.len() - MAX_INLINE_LIST
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Block I/O:  R {} / W {}",
        human_bytes(blkio_r),
        human_bytes(blkio_w)
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Esc close · Enter refresh · q quit",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// Render the "loading…" placeholder while `fetch_detail` is in flight.
pub fn render_loading(frame: &mut Frame) {
    let area = centered_rect(40, 20, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Loading… ")
        .borders(Borders::ALL);
    let para = Paragraph::new(Line::from(Span::styled(
        "Inspecting container…",
        Style::default().add_modifier(Modifier::ITALIC),
    )))
    .block(block);
    frame.render_widget(para, area);
}

/// Compute a percentage-sized centered rectangle.
///
/// `pct_x` / `pct_y` are 0..=100. The result is the inner rect: `area` minus
/// outer margins so the popup sits at the center of the frame with the given
/// width/height proportions.
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Map [`HealthSummary`] to a one-line summary for the popup.
fn health_str(h: &HealthSummary) -> String {
    match h {
        HealthSummary::None => "none".to_string(),
        HealthSummary::Starting => "starting".to_string(),
        HealthSummary::Healthy => "healthy".to_string(),
        HealthSummary::Unhealthy {
            failing_streak,
            last_output,
        } => {
            if last_output.is_empty() {
                format!("unhealthy (streak={failing_streak})")
            } else {
                // Keep the line tight: truncate the last_output to 40 chars
                // and strip newlines so the popup line never wraps unexpectedly.
                let trimmed: String = last_output
                    .replace(['\n', '\r'], " ")
                    .chars()
                    .take(40)
                    .collect();
                format!("unhealthy (streak={failing_streak}): {trimmed}")
            }
        }
    }
}

/// Comma-join a port list for the popup. Truncates after [`MAX_INLINE_LIST`]
/// with a "…N more" tail. Empty list -> "(none)".
fn format_ports(ports: &[PortSummary]) -> String {
    if ports.is_empty() {
        return "(none)".to_string();
    }
    let mut parts: Vec<String> = ports
        .iter()
        .take(MAX_INLINE_LIST)
        .map(|p| match p.public {
            Some(pub_port) => format!("{}/{} -> {}", p.private, p.proto.as_str(), pub_port),
            None => format!("{}/{}", p.private, p.proto.as_str()),
        })
        .collect();
    if ports.len() > MAX_INLINE_LIST {
        parts.push(format!("…{} more", ports.len() - MAX_INLINE_LIST));
    }
    parts.join(", ")
}

/// Render one mount as `source:destination` (skipping empty source for
/// anonymous tmpfs). Length-capped so multi-mount popups stay legible.
fn format_mount(m: &MountSummary) -> String {
    let src = if m.source.is_empty() {
        "(anon)"
    } else {
        m.source.as_str()
    };
    let body = format!("{src}:{}", m.destination);
    // Keep one mount line under ~60 chars so the popup doesn't wrap. Take
    // first/last halves so both endpoints stay visible on long bind paths.
    if body.chars().count() > 60 {
        let half = 28;
        let head: String = body.chars().take(half).collect();
        let tail: String = body.chars().rev().take(half).collect::<String>().chars().rev().collect();
        format!("{head}…{tail}")
    } else {
        body
    }
}

/// Human-readable byte count. 0 -> "0 B"; 1500 -> "1.5 KB"; 1_500_000 ->
/// "1.5 MB"; SI factor 1000 (matches `docker stats` output, which also uses
/// base-10 prefixes — the user-facing convention here).
fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    if n == 0 {
        return "0 B".to_string();
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", n, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::{PortProto, PortSummary};
    use crate::theme::Status;

    /// centered_rect math: a 60×60% rect inside a 100×100 area must land in
    /// the middle 60% of each axis (cols 20..80 / rows 20..80).
    #[test]
    fn centered_rect_centers_correctly() {
        let area = Rect::new(0, 0, 100, 100);
        let r = centered_rect(60, 60, area);
        assert_eq!(r.width, 60);
        assert_eq!(r.height, 60);
        assert_eq!(r.x, 20);
        assert_eq!(r.y, 20);
    }

    /// centered_rect handles small areas without overflow (clipped, not panicking).
    #[test]
    fn centered_rect_handles_small_area() {
        let area = Rect::new(0, 0, 10, 10);
        let r = centered_rect(60, 60, area);
        assert!(r.width <= 10 && r.height <= 10);
    }

    /// health_str covers all four variants.
    #[test]
    fn health_str_covers_all_variants() {
        assert_eq!(health_str(&HealthSummary::None), "none");
        assert_eq!(health_str(&HealthSummary::Starting), "starting");
        assert_eq!(health_str(&HealthSummary::Healthy), "healthy");
        let u = HealthSummary::Unhealthy {
            failing_streak: 3,
            last_output: String::new(),
        };
        assert_eq!(health_str(&u), "unhealthy (streak=3)");
        let u2 = HealthSummary::Unhealthy {
            failing_streak: 7,
            last_output: "exit code 1".to_string(),
        };
        assert_eq!(health_str(&u2), "unhealthy (streak=7): exit code 1");
    }

    /// Unhealthy last_output is normalized: newlines stripped, length capped.
    #[test]
    fn health_str_strips_newlines_and_caps_length() {
        let u = HealthSummary::Unhealthy {
            failing_streak: 1,
            last_output: "first line\nsecond line\nthird line — and this is going to overflow the limit easily".to_string(),
        };
        let s = health_str(&u);
        // Must NOT contain newlines.
        assert!(!s.contains('\n'));
        assert!(!s.contains('\r'));
        // The trimmed portion is capped at 40 chars after the prefix; the
        // total line still has the streak prefix on top, so check the body.
        let body = s.strip_prefix("unhealthy (streak=1): ").expect("prefix");
        assert!(body.chars().count() <= 40, "body too long: {body:?}");
    }

    /// human_bytes rounds and labels correctly across SI units.
    #[test]
    fn human_bytes_rounds_correctly() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(1500), "1.5 KB");
        assert_eq!(human_bytes(1_500_000), "1.5 MB");
        assert_eq!(human_bytes(2_500_000_000), "2.5 GB");
    }

    /// format_ports on empty list returns "(none)".
    #[test]
    fn format_ports_handles_empty_vec() {
        assert_eq!(format_ports(&[]), "(none)");
    }

    /// format_ports joins entries with comma and adds "…N more" past the cap.
    #[test]
    fn format_ports_truncates_past_inline_cap() {
        let ports: Vec<PortSummary> = (0..5)
            .map(|i| PortSummary {
                private: 8000 + i,
                public: Some(80 + i),
                proto: PortProto::Tcp,
            })
            .collect();
        let s = format_ports(&ports);
        // First MAX_INLINE_LIST entries present; the rest folded.
        assert!(s.starts_with("8000/tcp -> 80"));
        assert!(s.contains("…2 more"));
    }

    /// Port without a public mapping renders as just "private/proto".
    #[test]
    fn format_ports_no_public_port() {
        let ports = vec![PortSummary {
            private: 53,
            public: None,
            proto: PortProto::Udp,
        }];
        assert_eq!(format_ports(&ports), "53/udp");
    }

    /// format_mount truncates very long bind paths but keeps both endpoints.
    #[test]
    fn format_mount_truncates_long_paths() {
        let m = MountSummary {
            kind: "bind".to_string(),
            source: "/a/very/long/host/path/that/should/get/truncated/in/the/middle".to_string(),
            destination: "/app/data".to_string(),
            rw: true,
        };
        let s = format_mount(&m);
        // Contains an ellipsis (long line was folded).
        assert!(s.contains('…'), "long mount must fold with ellipsis: {s}");
        // Both endpoints survive.
        assert!(s.starts_with("/a/"), "head must be preserved: {s}");
        assert!(s.ends_with("/app/data"), "tail must be preserved: {s}");
    }

    /// Anonymous mount (empty source) renders as "(anon):destination".
    #[test]
    fn format_mount_anonymous_source() {
        let m = MountSummary {
            kind: "tmpfs".to_string(),
            source: String::new(),
            destination: "/tmp".to_string(),
            rw: true,
        };
        assert_eq!(format_mount(&m), "(anon):/tmp");
    }

    /// Smoke test: rendering the popup into a small Buffer doesn't panic;
    /// the container name appears in the top border line.
    #[test]
    fn render_detail_panel_does_not_panic_smoke() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let snap = DetailSnapshot {
            id: "abc".to_string(),
            name: "test-container".to_string(),
            status: Status::Running,
            health: HealthSummary::Healthy,
            started_at_iso: "2026-05-29T00:00:00Z".to_string(),
            restart_count: 2,
            restart_policy: "unless-stopped".to_string(),
            image_human: "nginx:latest".to_string(),
            image_digest: "sha256:abcd".to_string(),
            network_mode: "bridge".to_string(),
            networks: vec![("bridge".to_string(), "172.18.0.2".to_string())],
            ports: vec![PortSummary {
                private: 80,
                public: Some(8080),
                proto: PortProto::Tcp,
            }],
            mounts: vec![MountSummary {
                kind: "bind".to_string(),
                source: "/host/data".to_string(),
                destination: "/data".to_string(),
                rw: true,
            }],
        };
        term.draw(|f| render_detail_panel(f, &snap, 1500, 3500))
            .unwrap();
        // The popup must mention the container name somewhere on the frame
        // (in the bordered title). Walk the buffer for the substring.
        let buffer = term.backend().buffer().clone();
        let mut joined = String::new();
        for row in 0..buffer.area().height {
            for col in 0..buffer.area().width {
                joined.push_str(buffer[(col, row)].symbol());
            }
            joined.push('\n');
        }
        assert!(
            joined.contains("test-container"),
            "container name must appear in popup: {joined}"
        );
        assert!(joined.contains("nginx:latest"));
        assert!(joined.contains("8080"));
        assert!(joined.contains("1.5 KB"), "block I/O R must render: {joined}");
        assert!(joined.contains("3.5 KB"), "block I/O W must render: {joined}");
    }
}
