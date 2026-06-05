#[cfg(feature = "real-tauri")]
fn main() {
    real_tauri::run();
}

#[cfg(not(feature = "real-tauri"))]
fn main() {
    if let Err(error) = bridge_shell::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "real-tauri"))]
mod bridge_shell {
    use refine_native::core::supervisor::runtime::{DEFAULT_APP_ID, RuntimeRoot};
    use refine_native::surfaces::desktop::{
        DesktopShellBridge, FileDesktopShellBridge, desktop_shell_manifest,
    };

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let port = desktop_port();
        let local_url = format!("http://127.0.0.1:{port}");
        let runtime_root = desktop_runtime_root();
        let bridge = FileDesktopShellBridge::new(runtime_root, port);
        bridge.open_webview(&local_url)?;
        bridge.tray_menu_action("show")?;
        let manifest = desktop_shell_manifest(&local_url)?;
        println!("{}", serde_json::to_string_pretty(&manifest)?);
        Ok(())
    }

    fn desktop_port() -> u16 {
        std::env::var("REFINE_DESKTOP_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port > 0)
            .unwrap_or(8080)
    }

    fn desktop_runtime_root() -> std::path::PathBuf {
        std::env::var_os("REFINE_RUNTIME_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| RuntimeRoot::installed_user(DEFAULT_APP_ID).root)
    }
}

#[cfg(feature = "real-tauri")]
mod real_tauri {
    use refine_native::core::supervisor::runtime::{DEFAULT_APP_ID, RuntimeRoot};
    use refine_native::surfaces::desktop::{
        DesktopShellBridge, FileDesktopShellBridge, desktop_shell_manifest,
    };
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    #[tauri::command]
    fn desktop_manifest(port: u16) -> Result<serde_json::Value, String> {
        let local_url = format!("http://127.0.0.1:{port}");
        desktop_shell_manifest(&local_url)
            .map_err(|error| error.to_string())
            .and_then(|manifest| serde_json::to_value(manifest).map_err(|error| error.to_string()))
    }

    #[tauri::command]
    fn desktop_notify(
        runtime_root: String,
        port: u16,
        title: String,
        body: String,
    ) -> Result<(), String> {
        FileDesktopShellBridge::new(runtime_root, port)
            .notify(&title, &body)
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    fn desktop_deep_link(runtime_root: String, port: u16, link: String) -> Result<(), String> {
        FileDesktopShellBridge::new(runtime_root, port)
            .handle_deep_link(&link)
            .map_err(|error| error.to_string())
    }

    pub fn run() {
        let port = desktop_port();
        let runtime_root = desktop_runtime_root();
        let local_url = format!("http://127.0.0.1:{port}");
        let bridge = FileDesktopShellBridge::new(&runtime_root, port);
        let _ = bridge.open_webview(&local_url);

        tauri::Builder::default()
            .setup(move |app| {
                let menu = Menu::with_items(
                    app,
                    &[
                        &MenuItem::with_id(app, "show", "Show Refine", true, None::<&str>)?,
                        &MenuItem::with_id(
                            app,
                            "open_browser",
                            "Open in Browser",
                            true,
                            None::<&str>,
                        )?,
                        &MenuItem::with_id(
                            app,
                            "restart_daemon",
                            "Restart Daemon",
                            true,
                            None::<&str>,
                        )?,
                        &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
                    ],
                )?;
                let _tray = TrayIconBuilder::with_id("refine")
                    .tooltip("Refine")
                    .menu(&menu)
                    .on_menu_event(move |_app, event| {
                        let bridge = FileDesktopShellBridge::new(&runtime_root, port);
                        let _ = bridge.tray_menu_action(event.id().as_ref());
                    })
                    .build(app)?;
                WebviewWindowBuilder::new(
                    app,
                    "main",
                    WebviewUrl::External(local_url.parse().map_err(|error| {
                        tauri::Error::InvalidArgs(format!("invalid local URL: {error}"))
                    })?),
                )
                .title("Refine")
                .build()?;
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                desktop_manifest,
                desktop_notify,
                desktop_deep_link
            ])
            .run(tauri::generate_context!())
            .expect("failed to run Refine desktop shell");
    }

    fn desktop_port() -> u16 {
        std::env::var("REFINE_DESKTOP_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port > 0)
            .unwrap_or(8080)
    }

    fn desktop_runtime_root() -> std::path::PathBuf {
        std::env::var_os("REFINE_RUNTIME_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| RuntimeRoot::installed_user(DEFAULT_APP_ID).root)
    }
}
