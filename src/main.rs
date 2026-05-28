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
    let docker_handle = docker::spawn_docker_tasks(docker, tx);

    // Backend selection: explicit --kitty / --braille override; otherwise
    // auto-detect — real pixels where the graphics protocol exists (kitty/ghostty/
    // wezterm), braille everywhere else (Alacritty, SSH, dumb terminals).
    let force_kitty = args.iter().any(|a| a == "--kitty");
    let force_braille = args.iter().any(|a| a == "--braille");
    let result: Result<()> = if force_kitty || (!force_braille && kitty::supports_kitty_graphics()) {
        kitty::run_kitty(rx)
    } else {
        let mut tui = Tui::new()?;
        tui.enter()?;
        let mut app = App::with_docker_rx(rx);
        let r = app.run(&mut tui).await;
        // Always restore on the clean-exit path too, regardless of run() result.
        tui.exit()?;
        r
    };

    // Tear down the Docker producer task so it doesn't outlive the renderer.
    docker_handle.abort();

    result
}
