#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Native setup follows https://github.com/emilk/eframe_template/src/main.rs.
use fastdiff_cli::{IpcServer, OpenRequest, READY_MESSAGE, forward};
use fastdiff_core::View;
use fastdiff_gui::{app::Fastdiff, benchmark::Benchmark, settings::SettingsStore};
use std::io::Write;

fn main() {
    if let Err(error) = run() {
        eprintln!("fastdiff-gui: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut request = None;
    let mut ready_stdio = false;
    let mut smoke = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--request") => {
                let json = args.next().ok_or("--request needs a JSON argument")?;
                request = Some(serde_json::from_str::<OpenRequest>(
                    json.to_str().ok_or("Request JSON is not UTF-8")?,
                )?);
            }
            Some("--ready-stdio") => ready_stdio = true,
            Some("--smoke-test") => smoke = true,
            Some("--help") => {
                println!(
                    "fastdiff-gui [--request JSON] [--smoke-test]\nUse fastdiff [REPOSITORY|FILE] [--file PATH] [--view all|unstaged|staged|unpushed] [--theme] [--color-theme NAME] for normal launches."
                );
                return Ok(());
            }
            _ => return Err(format!("Unknown GUI argument: {}", arg.to_string_lossy()).into()),
        }
    }
    let benchmark = Benchmark::new(smoke);
    let request = request.unwrap_or(OpenRequest {
        target: fastdiff_cli::OpenTarget::Repository,
        repository: std::env::current_dir()?.to_string_lossy().into_owned(),
        file: None,
        view: View::All,
        wsl: None,
        theme: false,
        color_theme: None,
    });
    let server = match IpcServer::bind()? {
        Some(server) => server,
        None => {
            if !forward(&request)? {
                return Err("Existing viewer closed during launch; run fastdiff again".into());
            }
            if ready_stdio {
                accepted()?;
            }
            return Ok(());
        }
    };
    // Measured runs and smoke tests must not read or write the user's settings.
    let store = if benchmark.is_measuring() {
        SettingsStore::memory()
    } else {
        SettingsStore::open_default()
    };
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 400.0])
            .with_title("fastdiff"),
        ..Default::default()
    };
    eframe::run_native(
        "fastdiff",
        options,
        Box::new(move |creation| {
            let app = Fastdiff::new(request, server, &creation.egui_ctx, benchmark, store);
            if ready_stdio {
                accepted()?;
            }
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

fn accepted() -> std::io::Result<()> {
    println!("{READY_MESSAGE}");
    std::io::stdout().flush()
}
