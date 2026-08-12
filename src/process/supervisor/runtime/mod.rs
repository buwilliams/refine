use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::project::PrimaryRuntime;

pub const DEFAULT_APP_ID: &str = "refine";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeRoot {
    pub root: PathBuf,
}

impl RuntimeRoot {
    pub fn checkout_local(repo_root: impl AsRef<Path>) -> Self {
        Self {
            root: repo_root.as_ref().join("run"),
        }
    }

    pub fn primary_path(&self) -> PathBuf {
        self.root.join("primary.json")
    }

    pub fn port_root(&self, port: u16) -> PathBuf {
        self.root.join(port.to_string())
    }

    pub fn cache_dir(&self, port: u16) -> PathBuf {
        self.port_root(port).join("cache")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeBootstrap {
    pub primary: PrimaryRuntime,
    pub runtime_root: PathBuf,
    pub instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDeployment {
    CheckoutLocal,
    UserInstall,
    SystemService,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOs {
    Macos,
    Windows,
    Linux,
    Other(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimePathLayout {
    pub deployment: RuntimeDeployment,
    pub os: RuntimeOs,
    pub app_support_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub service_metadata_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimePathInputs {
    pub home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub xdg_cache_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
}

impl RuntimePathLayout {
    pub fn checkout_local(repo_root: impl AsRef<Path>) -> Self {
        let runtime_root = RuntimeRoot::checkout_local(repo_root).root;
        Self {
            deployment: RuntimeDeployment::CheckoutLocal,
            os: current_runtime_os(),
            app_support_dir: runtime_root.clone(),
            cache_dir: runtime_root.join("cache"),
            logs_dir: runtime_root.join("logs"),
            service_metadata_path: None,
            runtime_root,
        }
    }

    /// Resolve an installed daemon without creating an OS-global product home.
    /// Runtime, cache, and logs stay under the checkout; only the service
    /// registration path follows platform requirements.
    pub fn checkout_for_os(
        checkout: impl AsRef<Path>,
        os: RuntimeOs,
        app_id: &str,
        inputs: RuntimePathInputs,
    ) -> Self {
        let checkout = checkout.as_ref();
        let mut layout = Self::checkout_local(checkout);
        layout.app_support_dir = checkout.to_path_buf();
        layout.os = os.clone();
        layout.deployment = RuntimeDeployment::SystemService;
        layout.service_metadata_path = platform_service_metadata_path(os, app_id, inputs);
        layout
    }

    /// Calculate a retired external layout for read-only legacy detection.
    /// This must never be used as a normal runtime/cache/log destination.
    pub fn legacy_external_for_os(os: RuntimeOs, app_id: &str, inputs: RuntimePathInputs) -> Self {
        let app_id = app_id.trim();
        let app_id = if app_id.is_empty() {
            DEFAULT_APP_ID
        } else {
            app_id
        };
        match os {
            RuntimeOs::Macos => {
                let home = inputs.home.unwrap_or_else(|| PathBuf::from("."));
                let app_support_dir = home
                    .join("Library")
                    .join("Application Support")
                    .join(app_id);
                Self {
                    deployment: RuntimeDeployment::UserInstall,
                    os: RuntimeOs::Macos,
                    runtime_root: app_support_dir.join("run"),
                    cache_dir: home.join("Library").join("Caches").join(app_id),
                    logs_dir: home.join("Library").join("Logs").join(app_id),
                    service_metadata_path: Some(
                        home.join("Library")
                            .join("LaunchAgents")
                            .join(format!("com.{app_id}.daemon.plist")),
                    ),
                    app_support_dir,
                }
            }
            RuntimeOs::Windows => {
                let local = inputs
                    .local_app_data
                    .or(inputs.app_data)
                    .or(inputs.program_data)
                    .unwrap_or_else(|| PathBuf::from("."));
                let app_support_dir = local.join(app_id);
                Self {
                    deployment: RuntimeDeployment::UserInstall,
                    os: RuntimeOs::Windows,
                    runtime_root: app_support_dir.join("run"),
                    cache_dir: app_support_dir.join("cache"),
                    logs_dir: app_support_dir.join("logs"),
                    service_metadata_path: Some(app_support_dir.join("service.json")),
                    app_support_dir,
                }
            }
            RuntimeOs::Linux => {
                let home = inputs.home.unwrap_or_else(|| PathBuf::from("."));
                let state_root = inputs
                    .xdg_state_home
                    .unwrap_or_else(|| home.join(".local").join("state"));
                let cache_root = inputs.xdg_cache_home.unwrap_or_else(|| home.join(".cache"));
                let config_root = inputs
                    .xdg_config_home
                    .unwrap_or_else(|| home.join(".config"));
                let app_support_dir = state_root.join(app_id);
                Self {
                    deployment: RuntimeDeployment::UserInstall,
                    os: RuntimeOs::Linux,
                    runtime_root: app_support_dir.join("run"),
                    cache_dir: cache_root.join(app_id),
                    logs_dir: app_support_dir.join("logs"),
                    service_metadata_path: Some(
                        config_root
                            .join("systemd")
                            .join("user")
                            .join(format!("{app_id}.service")),
                    ),
                    app_support_dir,
                }
            }
            RuntimeOs::Other(name) => {
                let home = inputs.home.unwrap_or_else(|| PathBuf::from("."));
                let app_support_dir = home.join(format!(".{app_id}"));
                Self {
                    deployment: RuntimeDeployment::UserInstall,
                    os: RuntimeOs::Other(name),
                    runtime_root: app_support_dir.join("run"),
                    cache_dir: app_support_dir.join("cache"),
                    logs_dir: app_support_dir.join("logs"),
                    service_metadata_path: None,
                    app_support_dir,
                }
            }
        }
    }
}

fn platform_service_metadata_path(
    os: RuntimeOs,
    app_id: &str,
    inputs: RuntimePathInputs,
) -> Option<PathBuf> {
    let app_id = if app_id.trim().is_empty() {
        DEFAULT_APP_ID
    } else {
        app_id.trim()
    };
    match os {
        RuntimeOs::Macos => inputs.home.map(|home| {
            home.join("Library/LaunchAgents")
                .join(format!("com.{app_id}.daemon.plist"))
        }),
        RuntimeOs::Windows => inputs
            .local_app_data
            .or(inputs.app_data)
            .or(inputs.program_data)
            .map(|root| root.join(app_id).join("service.json")),
        RuntimeOs::Linux => inputs
            .xdg_config_home
            .or_else(|| inputs.home.map(|home| home.join(".config")))
            .map(|root| root.join("systemd/user").join(format!("{app_id}.service"))),
        RuntimeOs::Other(_) => None,
    }
}

impl RuntimePathInputs {
    pub fn from_env() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            app_data: std::env::var_os("APPDATA").map(PathBuf::from),
            program_data: std::env::var_os("PROGRAMDATA").map(PathBuf::from),
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
            xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        }
    }
}

pub fn current_runtime_os() -> RuntimeOs {
    match std::env::consts::OS {
        "macos" => RuntimeOs::Macos,
        "windows" => RuntimeOs::Windows,
        "linux" => RuntimeOs::Linux,
        other => RuntimeOs::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_root_preserves_checkout_local_shape() {
        let root = RuntimeRoot::checkout_local("/repo/refine");
        assert_eq!(
            root.primary_path(),
            PathBuf::from("/repo/refine/run/primary.json")
        );
        assert_eq!(
            root.cache_dir(8080),
            PathBuf::from("/repo/refine/run/8080/cache")
        );

        let layout = RuntimePathLayout::checkout_local("/repo/refine");
        assert_eq!(layout.runtime_root, PathBuf::from("/repo/refine/run"));
        assert_eq!(layout.deployment, RuntimeDeployment::CheckoutLocal);
    }

    #[test]
    fn runtime_path_layout_models_os_specific_user_installs() {
        let mac = RuntimePathLayout::legacy_external_for_os(
            RuntimeOs::Macos,
            "refine",
            RuntimePathInputs {
                home: Some(PathBuf::from("/Users/buddy")),
                ..Default::default()
            },
        );
        assert_eq!(
            mac.runtime_root,
            PathBuf::from("/Users/buddy/Library/Application Support/refine/run")
        );
        assert_eq!(
            mac.service_metadata_path.unwrap(),
            PathBuf::from("/Users/buddy/Library/LaunchAgents/com.refine.daemon.plist")
        );

        let windows = RuntimePathLayout::legacy_external_for_os(
            RuntimeOs::Windows,
            "refine",
            RuntimePathInputs {
                local_app_data: Some(PathBuf::from(r"C:\Users\buddy\AppData\Local")),
                ..Default::default()
            },
        );
        assert_eq!(
            windows.runtime_root,
            PathBuf::from(r"C:\Users\buddy\AppData\Local").join("refine/run")
        );

        let linux = RuntimePathLayout::legacy_external_for_os(
            RuntimeOs::Linux,
            "refine",
            RuntimePathInputs {
                home: Some(PathBuf::from("/home/buddy")),
                ..Default::default()
            },
        );
        assert_eq!(
            linux.runtime_root,
            PathBuf::from("/home/buddy/.local/state/refine/run")
        );
        assert_eq!(
            linux.service_metadata_path.unwrap(),
            PathBuf::from("/home/buddy/.config/systemd/user/refine.service")
        );
    }

    #[test]
    fn installed_daemon_keeps_product_state_checkout_local() {
        let layout = RuntimePathLayout::checkout_for_os(
            "/opt/refine",
            RuntimeOs::Linux,
            DEFAULT_APP_ID,
            RuntimePathInputs {
                home: Some(PathBuf::from("/home/buddy")),
                xdg_state_home: Some(PathBuf::from("/external/state")),
                xdg_cache_home: Some(PathBuf::from("/external/cache")),
                ..Default::default()
            },
        );
        assert_eq!(layout.runtime_root, PathBuf::from("/opt/refine/run"));
        assert_eq!(layout.cache_dir, PathBuf::from("/opt/refine/run/cache"));
        assert_eq!(layout.logs_dir, PathBuf::from("/opt/refine/run/logs"));
        assert_eq!(layout.app_support_dir, PathBuf::from("/opt/refine"));
        assert_eq!(
            layout.service_metadata_path,
            Some(PathBuf::from(
                "/home/buddy/.config/systemd/user/refine.service"
            ))
        );
    }
}
