use super::*;

use crate::tools::product::development_requests::{
    DevelopmentRequestSettings, FileDevelopmentRequestService, load_self_development_email_config,
    self_development_email_target_is_active,
};

pub(super) fn run_development_request_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut next_poll = Instant::now();
    let mut interval = Duration::from_secs(60);
    loop {
        if Instant::now() >= next_poll {
            let config = match load_self_development_email_config(runtime_root) {
                Ok(Some(config)) => config,
                Ok(None) => return Ok(()),
                Err(error) => {
                    eprintln!("refine development requests: invalid local contract: {error}");
                    next_poll = Instant::now() + interval;
                    thread::sleep(DEVELOPMENT_REQUEST_POLL_INTERVAL);
                    continue;
                }
            };
            interval = Duration::from_secs(config.poll_seconds);
            match current_target_root(runtime_root, project_registry_root) {
                Ok(Some(target_root))
                    if self_development_email_target_is_active(&config, &target_root)? =>
                {
                    let result = (|| {
                        let refine_dir = refine_dir_for_target_root(&target_root)?;
                        let settings = DevelopmentRequestSettings::from_local_config(&config);
                        FileDevelopmentRequestService::new(runtime_root, refine_dir, &target_root)
                            .process_once(&settings)
                    })();
                    if let Err(error) = result {
                        eprintln!("refine development requests: {error}");
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(error) => {
                    eprintln!("refine development requests: failed to resolve active app: {error}")
                }
            }
            next_poll = Instant::now() + interval;
        }
        thread::sleep(DEVELOPMENT_REQUEST_POLL_INTERVAL);
    }
}
