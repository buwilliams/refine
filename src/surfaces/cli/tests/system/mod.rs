mod commands;
mod daemon;
mod installation;
mod release_workers;

use super::*;

fn write_installed_binary(checkout: &std::path::Path) {
    let executable = checkout.join("bin/refine");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "installed fixture\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }
}
