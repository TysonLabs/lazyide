use std::io::{self, Stdout};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

mod app;
mod keybinds;
mod lsp_client;
mod persistence;
mod syntax;
mod tab;
mod theme;
mod tree_item;
mod types;
mod ui;
mod util;
use app::App;
use lsp_client::resolve_rust_analyzer_bin;
use ui::draw;

pub fn run() -> io::Result<()> {
    if std::env::args().any(|a| a == "--setup") {
        return run_setup();
    }

    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("Usage: lazyide [OPTIONS] [PATH]");
        println!();
        println!("Arguments:");
        println!("  [PATH]    Directory to open (default: current directory)");
        println!();
        println!("Options:");
        println!("  --setup   Check for and install optional tools (rust-analyzer, ripgrep)");
        println!("  --help    Show this help message");
        return Ok(());
    }

    let root = if let Some(path) = std::env::args().nth(1) {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?
    };
    if !root.is_dir() {
        eprintln!("Root path is not a directory: {}", root.display());
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;

    let enhanced_keys =
        ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced_keys {
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        );
        original_hook(info);
    }));

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(root)?;
    app.enhanced_keys = enhanced_keys;
    let result = run_app(terminal, app);

    disable_raw_mode()?;
    let mut stdout = io::stdout();
    if enhanced_keys {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;

    result
}

fn run_app(mut terminal: Terminal<CrosstermBackend<Stdout>>, mut app: App) -> io::Result<()> {
    loop {
        app.poll_lsp();
        app.poll_git_results();
        app.poll_wrap_rebuild();
        if let Err(err) = app.poll_fs_changes() {
            app.set_status(format!("Filesystem update error: {err}"));
        }
        if let Err(err) = app.poll_autosave() {
            app.set_status(format!("Autosave error: {err}"));
        }
        app.update_status_for_cursor();
        terminal.draw(|f| draw(&mut app, f))?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100))? {
            // Drain all pending events before the next draw to avoid
            // queuing hundreds of redraws during rapid mouse scrolling.
            loop {
                let ev = event::read()?;
                match ev {
                    Event::Key(key) => {
                        if let Err(err) = app.handle_key(key) {
                            app.set_status(format!("Action failed: {err}"));
                        }
                    }
                    Event::Mouse(mouse) => {
                        if let Err(err) = app.handle_mouse(mouse) {
                            app.set_status(format!("Action failed: {err}"));
                        }
                    }
                    Event::Paste(text) => {
                        app.handle_paste(text);
                    }
                    _ => {}
                }
                if app.quit {
                    return Ok(());
                }
                // If no more events are pending, break and redraw.
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
}

fn run_setup() -> io::Result<()> {
    println!("lazyide setup\n");

    let has_ra = resolve_rust_analyzer_bin().is_some();
    let has_rg = Command::new("rg").arg("--version").output().is_ok();

    if has_ra {
        println!("  [ok] rust-analyzer found");
    } else {
        println!("  [missing] rust-analyzer not found");
        println!("    -> rustup component add rust-analyzer");
    }
    if has_rg {
        println!("  [ok] ripgrep (rg) found");
    } else {
        println!("  [missing] ripgrep (rg) not found");
        if cfg!(target_os = "macos") {
            println!("    -> brew install ripgrep");
        } else {
            println!("    -> cargo install ripgrep");
        }
    }

    if has_ra && has_rg {
        println!("\nAll tools installed. You are good to go!");
        return Ok(());
    }

    println!("\nInstall missing tools? [y/N] ");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        return Ok(());
    }

    if !has_ra {
        println!("\nInstalling rust-analyzer...");
        let status = Command::new("rustup")
            .args(["component", "add", "rust-analyzer"])
            .status();
        match status {
            Ok(s) if s.success() => println!("  [ok] rust-analyzer installed"),
            _ => println!("  [failed] install manually: rustup component add rust-analyzer"),
        }
    }

    if !has_rg {
        println!("\nInstalling ripgrep...");
        let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
            ("brew", &["install", "ripgrep"])
        } else {
            ("cargo", &["install", "ripgrep"])
        };
        let status = Command::new(cmd).args(args).status();
        match status {
            Ok(s) if s.success() => println!("  [ok] ripgrep installed"),
            _ => println!("  [failed] install manually: cargo install ripgrep"),
        }
    }

    println!("\nSetup complete!");
    Ok(())
}
