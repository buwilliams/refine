fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    refine::infrastructure::process::subprocess::owned_groups::run_scope_guardian_if_requested();
    match refine::surfaces::cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
