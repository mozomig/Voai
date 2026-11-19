#[cfg(target_os = "macos")]
mod app;
#[cfg(any(target_os = "macos", test))]
mod insertion;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod ui;

use std::process::ExitCode;

const HELP: &str = "voai                 Открыть настройки и включить глобальную диктовку\nvoai probe-input     Проверить прямой ввод фразы «Тест Voai 🎙» в редакторе";

#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
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
                    .ok_or("Укажите PID тестового приложения")?,
            ),
            _ => return Err(HELP.into()),
        };
        if args.next().is_some() {
            return Err(HELP.into());
        }
        return macos::probe(pid);
    }
    if args.next().is_some() {
        return Err(HELP.into());
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
            println!("{HELP}");
            Ok(())
        }
        _ => Err(HELP.into()),
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
        eprintln!("Для Voai нужна macOS.\n{HELP}");
        ExitCode::FAILURE
    }
}
