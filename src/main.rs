#[cfg(target_os = "macos")]
mod app;
#[cfg(any(target_os = "macos", test))]
mod insertion;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod ui;

use std::process::ExitCode;
#[cfg(target_os = "macos")]
use voai_l10n::tr;

#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    if command.as_deref() == Some(std::ffi::OsStr::new("--file-worker")) {
        return voai_speech::file_worker_main();
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("--speech-worker")) {
        return voai_speech::worker_main();
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("probe-input")) {
        let pid = match args.next() {
            None => None,
            Some(flag) if flag == "--pid" => Some(
                args.next()
                    .and_then(|a| a.to_str()?.parse::<i32>().ok())
                    .filter(|pid| *pid > 0)
                    .ok_or(tr("app.cli.pid"))?,
            ),
            _ => return Err(tr("app.cli.help").into()),
        };
        if args.next().is_some() {
            return Err(tr("app.cli.help").into());
        }
        return macos::probe(pid);
    }
    if args.next().is_some() {
        return Err(tr("app.cli.help").into());
    }
    match command.as_deref().and_then(|s| s.to_str()) {
        None if command.is_none() => {
            ui::run(app::Backend::new()?);
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("Voai {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") => {
            println!("{}", tr("app.cli.help"));
            Ok(())
        }
        _ => Err(tr("app.cli.help").into()),
    }
}

fn main() -> ExitCode {
    #[cfg(target_os = "macos")]
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Voai requires macOS.");
        ExitCode::FAILURE
    }
}
