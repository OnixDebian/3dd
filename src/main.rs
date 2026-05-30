mod action;
mod app;
mod camera;
mod config;
mod docker;
mod kitty;
mod render3d;
mod theme;
mod tui;
mod ui;
mod world;

use color_eyre::Result;
use tokio::sync::mpsc;

use crate::app::App;
use crate::tui::Tui;
use crate::world::DockerMsg;

/// Install color-eyre's panic/error reporting plus a terminal-restoring panic
/// hook. The hook MUST run before we ever enter raw mode so that any panic on
/// any code path leaves the terminal usable (Pitfall #11).
fn install_hooks() -> Result<()> {
    // color-eyre builds the (panic_hook, eyre_hook) pair.
    let (panic_hook, eyre_hook) = color_eyre::config::HookBuilder::default()
        .into_hooks();
    eyre_hook.install()?;

    // Wrap the existing panic hook so the terminal is restored FIRST, then the
    // pretty color-eyre panic report is printed to a clean screen.
    let panic_hook = panic_hook.into_panic_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort restore; ignore errors because we are already panicking.
        let _ = tui::restore();
        panic_hook(info);
    }));

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    install_hooks()?;

    let args: Vec<String> = std::env::args().collect();

    // One-frame RGBA dump for offline inspection (no terminal needed).
    //
    // INTENTIONALLY DAEMON-FREE: this is an inspection tool used to verify the
    // RENDERER, not the data path. It always paints the synthetic scene so it
    // works on machines that don't run Docker (CI, fresh VMs). The live data
    // path is exercised by `--kitty` / `--braille` against a real daemon.
    if let Some(pos) = args.iter().position(|a| a == "--dump-rgba") {
        let path = args.get(pos + 1).map(String::as_str).unwrap_or("/tmp/dd3_kitty.rgba");
        return kitty::dump_rgba(path, 720, 560);
    }

    // Mixed-status dump: like --dump-rgba, but the scene mimics a real-world
    // mostly-stopped Docker rack (a few Running, many Stopped/Paused, spread
    // across several network groups). Used to verify the wireframe path
    // visually without needing a live daemon with that exact mix.
    if let Some(pos) = args.iter().position(|a| a == "--dump-mixed") {
        let path = args.get(pos + 1).map(String::as_str).unwrap_or("/tmp/dd3_mixed.rgba");
        return kitty::dump_rgba_mixed(path, 720, 560);
    }

    // Real-snapshot dump: shells out to `docker ps -a`, builds a scene using
    // the user's ACTUAL container names + networks + states (same layout
    // pipeline as the live render), then forces every other container to
    // Running with a synthetic load — so the test frame shows a realistic
    // mixed solid/wireframe rack. Read-only: never starts/stops anything.
    if let Some(pos) = args.iter().position(|a| a == "--dump-snapshot") {
        let path = args.get(pos + 1).map(String::as_str).unwrap_or("/tmp/dd3_snapshot.rgba");
        return kitty::dump_snapshot(path, 720, 560);
    }

    // THEME-06 v1: load runtime config (palette name, hud_visible, auto_degrade,
    // force_mode, degraded_fps_cap) from ~/.config/3dd/config.toml or the
    // --config <PATH> override. Same Pitfall 9 contract as the docker probe
    // below: a malformed-TOML / unreadable-config error MUST land on a CLEAN
    // terminal (no alt-screen yet, no raw mode yet) — eprintln + exit(1).
    //
    // 05-04 wires `config` into App::with_docker and run_kitty so the chosen
    // palette / HUD state / degrade policy actually drives rendering. THIS
    // plan (05-01) only validates the load: the binding is intentionally
    // discarded so the loader runs at startup but no downstream consumer
    // exists yet. The `let _ = config;` below is removed in 05-04.
    let config = match config::load_or_default_with_cli(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            std::process::exit(1);
        }
    };

    // ROB-01 / PITFALLS Pitfall 9: probe the Docker daemon BEFORE entering raw
    // mode (Tui::enter for braille, enable_raw_mode inside run_kitty for kitty).
    // On failure we print the actionable ProbeError to a CLEAN terminal and
    // exit non-zero — never a garbled alt-screen panic.
    let docker = match docker::connect_and_probe().await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Single mpsc channel: producer = spawn_docker_tasks; consumer = the chosen
    // backend's render loop. Unbounded matches the 03-03 contract; cadence is
    // decoupled because the backend drains with try_recv each pass (DOCK-04).
    let (tx, rx) = mpsc::unbounded_channel::<DockerMsg>();
    let test_mode = args.iter().any(|a| a == "--test");
    // In --test mode, also keep a clone of the sender so we can inject
    // synthetic Running + Stat messages for half the user's containers AFTER
    // the natural list_containers seed has populated the world. The live
    // event/stats streams keep working normally on the original tx.
    let test_tx = test_mode.then(|| tx.clone());
    // 04-06b: clone the docker handle + tx for the renderer's Effect::SpawnInspect
    // path (off-thread `fetch_detail` on Enter, results piped back through the
    // same mpsc). The producer task gets the ORIGINAL docker + tx, the renderer
    // gets the clones — both backends accept (docker, tx, rx[, handle]).
    let docker_for_render = docker.clone();
    let tx_for_inspect = tx.clone();
    let docker_handle = docker::spawn_docker_tasks(docker, tx);

    // --test: read the user's containers via `docker ps -a`, force every
    // other (alphabetical by name) to Running with a varied synthetic load,
    // and push the StatusChanged + Stat messages over the same mpsc that the
    // real producer uses. Read-only: never starts or stops a container.
    // Sleeps briefly first so the producer's initial Added messages land
    // before our overrides, otherwise StatusChanged would target an unknown
    // id and be dropped by the reconciler.
    if let Some(tx) = test_tx {
        tokio::spawn(async move {
            use std::time::Duration as StdDuration;

            use crate::docker::stats::StatSample;
            use crate::theme::Status;

            tokio::time::sleep(StdDuration::from_millis(750)).await;

            let out = tokio::process::Command::new("docker")
                .args([
                    "ps",
                    "-a",
                    "--no-trunc",
                    "--format",
                    "{{.ID}}|{{.Names}}",
                ])
                .output()
                .await;
            let Ok(out) = out else { return };
            if !out.status.success() {
                return;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut records: Vec<(String, String)> = stdout
                .lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.splitn(2, '|').collect();
                    (parts.len() == 2 && !parts[0].is_empty())
                        .then(|| (parts[0].to_string(), parts[1].to_string()))
                })
                .collect();
            records.sort_by(|a, b| a.1.cmp(&b.1));

            let forced: Vec<&(String, String)> = records.iter().step_by(2).collect();
            let n = forced.len().max(1) as f32;
            for (i, (id, _)) in forced.iter().enumerate() {
                let _ = tx.send(DockerMsg::StatusChanged(id.clone(), Status::Running));
                let load = 0.3 + 0.5 * (i as f32 / n);
                let sample = StatSample {
                    cpu_pct: 0.0,
                    mem_used: 0,
                    mem_limit: 0,
                    mem_fraction: 0.0,
                    load,
                    warming_up: false,
                    blkio_r_bytes: 0,
                    blkio_w_bytes: 0,
                };
                let _ = tx.send(DockerMsg::Stat(id.clone(), sample));
            }

            // Smooth "breathing": Stat updates at ~20Hz (every 50ms) so the
            // size sweep is sub-pixel between updates instead of jumping at a
            // visible cadence. Cheap (a few hundred messages/sec total across
            // ~7 forced boxes), well under the LiveWorld dedup path
            // ("same load -> no rebuild"). Amplitude is gentler than the
            // initial seed: load varies in [0.40, 0.70] so the size swing
            // reads as breathing, not strobing.
            let mut tick = tokio::time::interval(StdDuration::from_millis(50));
            tick.tick().await; // skip the immediate first tick
            let start = std::time::Instant::now();
            loop {
                tick.tick().await;
                let t = start.elapsed().as_secs_f32();
                for (i, (id, _)) in forced.iter().enumerate() {
                    // Sinusoid in [0.40, 0.70] with a per-box phase offset so
                    // each box breathes a bit out of sync with its neighbors.
                    let phase = i as f32 * 0.6;
                    let load = 0.55 + 0.15 * (t * 0.8 + phase).sin();
                    let sample = StatSample {
                        cpu_pct: 0.0,
                        mem_used: 0,
                        mem_limit: 0,
                        mem_fraction: 0.0,
                        load,
                        warming_up: false,
                        blkio_r_bytes: 0,
                        blkio_w_bytes: 0,
                    };
                    if tx.send(DockerMsg::Stat(id.clone(), sample)).is_err() {
                        return; // receiver dropped — app exiting
                    }
                }
            }
        });
    }

    // Backend selection: explicit --kitty / --braille override; otherwise
    // auto-detect — real pixels where the graphics protocol exists (kitty/ghostty/
    // wezterm), braille everywhere else (Alacritty, SSH, dumb terminals).
    let force_kitty = args.iter().any(|a| a == "--kitty");
    let force_braille = args.iter().any(|a| a == "--braille");
    let result: Result<()> = if force_kitty || (!force_braille && kitty::supports_kitty_graphics()) {
        // run_kitty is sync but lives inside #[tokio::main] — Handle::current()
        // captures the active runtime so its inline `handle.spawn(...)` calls
        // (Effect::SpawnInspect) schedule onto the SAME runtime the producer
        // task already runs on.
        let handle = tokio::runtime::Handle::current();
        kitty::run_kitty(docker_for_render, tx_for_inspect, rx, handle, config)
    } else {
        let mut tui = Tui::new()?;
        tui.enter()?;
        let mut app = App::with_docker(docker_for_render, tx_for_inspect, rx, config);
        let r = app.run(&mut tui).await;
        // Always restore on the clean-exit path too, regardless of run() result.
        tui.exit()?;
        r
    };

    // Tear down the Docker producer task so it doesn't outlive the renderer.
    docker_handle.abort();

    result
}
