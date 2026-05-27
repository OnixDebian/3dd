mod action;
mod app;
mod camera;
mod config;
mod kitty;
mod render3d;
mod theme;
mod tui;
mod ui;
mod world;

use color_eyre::Result;

use crate::app::App;
use crate::tui::Tui;

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
    if let Some(pos) = args.iter().position(|a| a == "--dump-rgba") {
        let path = args.get(pos + 1).map(String::as_str).unwrap_or("/tmp/dd3_kitty.rgba");
        return kitty::dump_rgba(path, 720, 560);
    }

    // Backend selection: explicit --kitty / --braille override; otherwise
    // auto-detect — real pixels where the graphics protocol exists (kitty/ghostty/
    // wezterm), braille everywhere else (Alacritty, SSH, dumb terminals).
    let force_kitty = args.iter().any(|a| a == "--kitty");
    let force_braille = args.iter().any(|a| a == "--braille");
    if force_kitty || (!force_braille && kitty::supports_kitty_graphics()) {
        return kitty::run_kitty();
    }

    let mut tui = Tui::new()?;
    tui.enter()?;

    let mut app = App::new();
    let result = app.run(&mut tui).await;

    // Always restore on the clean-exit path too, regardless of run() result.
    tui.exit()?;

    result
}
