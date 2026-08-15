use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::process::supervisor::errors::{RefineError, RefineResult};

#[cfg(test)]
pub(crate) mod test_fixture;

pub const DEPLOYED_MARKER: &str = ".refine-deployed";
pub const INSTALL_RUNBOOK: &str = "docs/runbooks/install.md";
static ACTIVE_CHECKOUT_PATHS: OnceLock<RefineCheckoutPaths> = OnceLock::new();

/// Canonical product paths owned by one Refine checkout or gitless deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefineCheckoutPaths {
    pub checkout: PathBuf,
    pub binary: PathBuf,
    pub runtime_root: PathBuf,
}

impl RefineCheckoutPaths {
    /// Resolve the product home which owns the current executable. Caller CWD
    /// is deliberately not an input: a checkout-local binary must not adopt a
    /// second checkout merely because it was launched from there.
    pub fn discover() -> RefineResult<Self> {
        if let Some(paths) = ACTIVE_CHECKOUT_PATHS.get() {
            return Ok(paths.clone());
        }
        let executable = std::env::current_exe().map_err(|error| {
            RefineError::Io(format!(
                "failed to resolve the active Refine executable: {error}"
            ))
        })?;
        let paths = Self::for_invocation(&executable)?;
        let _ = ACTIVE_CHECKOUT_PATHS.set(paths.clone());
        Ok(ACTIVE_CHECKOUT_PATHS.get().cloned().unwrap_or(paths))
    }

    pub fn for_invocation(executable: &Path) -> RefineResult<Self> {
        let executable = canonical_existing(executable, "Refine executable")?;
        let Some(home) = executable.parent().and_then(find_product_home_from) else {
            return Err(RefineError::NotFound(format!(
                "could not identify the Refine product home which owns executable {}; invoke <checkout>/bin/refine or the checkout-owned ./r launcher; caller CWD, HOME, XDG, and OS app-support paths are not ownership authorities",
                executable.display()
            )));
        };
        Self::from_product_home(home)
    }

    pub fn from_product_home(home: impl AsRef<Path>) -> RefineResult<Self> {
        let home = canonical_existing(home.as_ref(), "Refine product home")?;
        validate_product_home_shape(&home)?;
        Ok(Self::from_validated_home(home))
    }

    pub fn from_source_checkout(checkout: impl AsRef<Path>) -> RefineResult<Self> {
        let paths = Self::from_product_home(checkout)?;
        paths.validate_source_checkout()?;
        Ok(paths)
    }

    pub fn from_runtime_root(runtime_root: &Path) -> RefineResult<Self> {
        let home = runtime_root.parent().ok_or_else(|| {
            RefineError::InvalidInput(format!(
                "runtime root {} has no product-home parent",
                runtime_root.display()
            ))
        })?;
        let paths = Self::from_product_home(home)?;
        if paths.runtime_root != runtime_root {
            return Err(RefineError::Conflict(format!(
                "runtime root {} does not match owning product runtime {}",
                runtime_root.display(),
                paths.runtime_root.display()
            )));
        }
        Ok(paths)
    }

    pub fn from_port_runtime_root(port_runtime_root: &Path) -> RefineResult<(Self, u16)> {
        let port = port_runtime_root
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| {
                RefineError::InvalidInput(format!(
                    "port runtime {} does not end in a valid port",
                    port_runtime_root.display()
                ))
            })?;
        let runtime_root = port_runtime_root.parent().ok_or_else(|| {
            RefineError::InvalidInput(format!(
                "port runtime {} has no base runtime parent",
                port_runtime_root.display()
            ))
        })?;
        let paths = Self::from_runtime_root(runtime_root)?;
        paths.require_port_runtime(port_runtime_root, port)?;
        Ok((paths, port))
    }

    fn from_validated_home(checkout: PathBuf) -> Self {
        Self {
            binary: checkout.join("bin/refine"),
            runtime_root: checkout.join("run"),
            checkout,
        }
    }

    /// Explicit constructor for isolated tests and internal adapters which do
    /// not represent a normal product invocation.
    #[cfg(test)]
    pub fn for_test(home: impl Into<PathBuf>) -> Self {
        Self::from_validated_home(home.into())
    }

    pub fn port_runtime_root(&self, port: u16) -> PathBuf {
        self.runtime_root.join(port.to_string())
    }

    /// Resolve an executable which belongs to this product home.
    ///
    /// Runtime ownership and executable selection are separate: an installed
    /// invocation runs `bin/refine`, while a source/debug invocation runs the
    /// checkout's active Cargo-built executable. Both use the same `run`
    /// directory, so child processes must inherit the active executable rather
    /// than deriving `bin/refine` from the runtime path.
    pub fn require_owned_executable(&self, executable: &Path) -> RefineResult<PathBuf> {
        let executable = canonical_existing(executable, "active Refine executable")?;
        let owner = Self::for_invocation(&executable)?;
        if owner.checkout != self.checkout {
            return Err(RefineError::Conflict(format!(
                "active Refine executable {} belongs to product home {}, but runtime {} belongs to product home {}",
                executable.display(),
                owner.checkout.display(),
                self.runtime_root.display(),
                self.checkout.display()
            )));
        }
        Ok(executable)
    }

    pub fn validate_source_checkout(&self) -> RefineResult<()> {
        if !is_source_checkout_shape(&self.checkout) {
            return Err(RefineError::Conflict(format!(
                "Refine product home {} is a gitless deployed installation; source status, checks, and promotion require the owning Git source checkout",
                self.checkout.display()
            )));
        }
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel", "--is-inside-work-tree"])
            .current_dir(&self.checkout)
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to validate Refine source checkout {}: {error}",
                    self.checkout.display()
                ))
            })?;
        if !output.status.success() {
            return Err(RefineError::Conflict(format!(
                "Refine product home {} is not a usable Git source checkout: {}",
                self.checkout.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let reported = lines.next().map(PathBuf::from).ok_or_else(|| {
            RefineError::Conflict("git did not report a source checkout root".to_string())
        })?;
        let inside = lines.next().unwrap_or_default();
        let reported = canonical_existing(&reported, "Git source checkout")?;
        if reported != self.checkout || inside != "true" {
            return Err(RefineError::Conflict(format!(
                "Git source identity {} does not match owning Refine product home {}",
                reported.display(),
                self.checkout.display()
            )));
        }
        Ok(())
    }

    pub fn resolve_runtime_compatibility(&self, requested: &Path) -> RefineResult<PathBuf> {
        if requested == Path::new("run") || requested == self.runtime_root {
            return Ok(self.runtime_root.clone());
        }
        let kind = if requested.is_absolute() {
            "absolute"
        } else {
            "relative"
        };
        Err(RefineError::Conflict(format!(
            "{kind} runtime root {} does not match the checkout-owned runtime {}; legacy external runtime trees are not merged or overwritten; use `run` or the exact canonical checkout runtime and run `refine system repair` for an existing external registration",
            requested.display(),
            self.runtime_root.display()
        )))
    }

    pub fn require_port_runtime(&self, runtime: &Path, port: u16) -> RefineResult<PathBuf> {
        let expected = self.port_runtime_root(port);
        if runtime == expected {
            Ok(expected)
        } else {
            Err(RefineError::Conflict(format!(
                "port runtime {} does not match owning checkout, port, and operation authority {}",
                runtime.display(),
                expected.display()
            )))
        }
    }

    pub fn require_same_source_checkout(&self, requested: &Path) -> RefineResult<PathBuf> {
        let requested = canonical_existing(requested, "requested source checkout")?;
        if requested != self.checkout {
            return Err(RefineError::Conflict(format!(
                "requested source checkout {} does not match the product home which owns this invocation {}",
                requested.display(),
                self.checkout.display()
            )));
        }
        self.validate_source_checkout()?;
        Ok(requested)
    }
}

pub fn discover_refine_checkout() -> RefineResult<PathBuf> {
    Ok(RefineCheckoutPaths::discover()?.checkout)
}

pub fn active_refine_paths() -> RefineResult<(PathBuf, PathBuf)> {
    let executable = std::env::current_exe().map_err(|error| {
        RefineError::Io(format!(
            "failed to resolve active Refine executable: {error}"
        ))
    })?;
    let executable = canonical_existing(&executable, "active Refine executable")?;
    let checkout = RefineCheckoutPaths::for_invocation(&executable)?.checkout;
    Ok((executable, checkout))
}

/// Compatibility predicate for existing callers. It recognizes both the
/// current source checkout and the runbook-defined gitless deployed product.
pub fn is_refine_checkout(path: &Path) -> bool {
    validate_product_home_shape(path).is_ok()
}

pub fn is_refine_source_checkout(path: &Path) -> bool {
    validate_common_release_files(path) && is_source_checkout_shape(path)
}

fn find_product_home_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|path| is_refine_checkout(path))
        .map(Path::to_path_buf)
}

fn validate_product_home_shape(path: &Path) -> RefineResult<()> {
    if !validate_common_release_files(path) {
        return Err(RefineError::NotFound(format!(
            "{} is not a Refine product home: required current release files Cargo.toml, src/main.rs, docs/runbooks/install.md, and r were not all found",
            path.display()
        )));
    }
    if is_source_checkout_shape(path) || is_deployed_product_shape(path) {
        return Ok(());
    }
    Err(RefineError::NotFound(format!(
        "{} is neither a valid Refine Git checkout nor a gitless deployment with .refine-deployed release_bin=bin/refine and an installed bin/refine",
        path.display()
    )))
}

fn validate_common_release_files(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        && path.join("src/main.rs").is_file()
        && path.join(INSTALL_RUNBOOK).is_file()
        && path.join("r").is_file()
}

fn is_source_checkout_shape(path: &Path) -> bool {
    let git = path.join(".git");
    if git.is_dir() {
        return true;
    }
    let Ok(text) = fs::read_to_string(git) else {
        return false;
    };
    let Some(gitdir) = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
    else {
        return false;
    };
    let gitdir = PathBuf::from(gitdir.trim());
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        path.join(gitdir)
    };
    gitdir.is_dir()
}

fn is_deployed_product_shape(path: &Path) -> bool {
    let Ok(marker) = fs::read_to_string(path.join(DEPLOYED_MARKER)) else {
        return false;
    };
    let mut mode = None;
    let mut release_bin = None;
    for line in marker.lines().filter(|line| !line.trim().is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        match key.trim() {
            "mode" if mode.is_none() => mode = Some(value.trim()),
            "release_bin" if release_bin.is_none() => release_bin = Some(value.trim()),
            _ => return false,
        }
    }
    mode == Some("deployed")
        && release_bin == Some("bin/refine")
        && path.join("bin/refine").is_file()
}

fn canonical_existing(path: &Path, description: &str) -> RefineResult<PathBuf> {
    path.canonicalize().map_err(|error| {
        RefineError::Io(format!(
            "failed to canonicalize {description} {}: {error}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "refine-checkout-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn common_shape(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs/runbooks")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname='refine'\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join(INSTALL_RUNBOOK), "# Install\n").unwrap();
        fs::write(root.join("r"), "#!/bin/sh\n").unwrap();
    }

    #[test]
    fn gitless_deployed_binary_owns_checkout_runtime_from_unrelated_cwd() {
        let root = temp_root("deployed");
        common_shape(&root);
        fs::write(root.join("bin/refine"), "binary\n").unwrap();
        fs::write(
            root.join(DEPLOYED_MARKER),
            "mode=deployed\nrelease_bin=bin/refine\n",
        )
        .unwrap();
        let paths = RefineCheckoutPaths::for_invocation(&root.join("bin/refine")).unwrap();
        assert_eq!(paths.checkout, root.canonicalize().unwrap());
        assert_eq!(paths.runtime_root, root.canonicalize().unwrap().join("run"));
        assert_eq!(
            paths.port_runtime_root(4557),
            root.canonicalize().unwrap().join("run/4557")
        );
        let error = paths.validate_source_checkout().unwrap_err().to_string();
        assert!(error.contains("gitless deployed"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linked_worktree_marker_and_non_default_ports_remain_isolated() {
        let root = temp_root("linked");
        common_shape(&root);
        let gitdir = root.join("common/worktrees/linked");
        fs::create_dir_all(&gitdir).unwrap();
        fs::write(root.join(".git"), "gitdir: common/worktrees/linked\n").unwrap();
        fs::write(root.join("bin/refine"), "binary\n").unwrap();
        let paths = RefineCheckoutPaths::for_invocation(&root.join("bin/refine")).unwrap();
        assert_ne!(
            paths.port_runtime_root(4557),
            paths.port_runtime_root(45570)
        );
        assert!(is_refine_source_checkout(&root));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_linked_worktree_passes_exact_git_source_validation() {
        let expected = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let paths = RefineCheckoutPaths::from_source_checkout(&expected).unwrap();
        assert_eq!(paths.checkout, expected);
        paths.require_same_source_checkout(&paths.checkout).unwrap();
    }

    #[test]
    fn near_miss_and_external_runtime_fail_closed() {
        let root = temp_root("near-miss");
        common_shape(&root);
        fs::write(root.join("bin/refine"), "binary\n").unwrap();
        let error = RefineCheckoutPaths::for_invocation(&root.join("bin/refine"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("caller CWD"));

        fs::create_dir(root.join(".git")).unwrap();
        let paths = RefineCheckoutPaths::for_invocation(&root.join("bin/refine")).unwrap();
        let error = paths
            .resolve_runtime_compatibility(Path::new("/tmp/external-refine-run"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not merged or overwritten"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_deployed_product_homes_never_share_runtime_or_binary_identity() {
        let left = temp_root("deployed-left");
        let right = temp_root("deployed-right");
        for root in [&left, &right] {
            common_shape(root);
            fs::write(root.join("bin/refine"), "binary\n").unwrap();
            fs::write(
                root.join(DEPLOYED_MARKER),
                "mode=deployed\nrelease_bin=bin/refine\n",
            )
            .unwrap();
        }

        let left_paths = RefineCheckoutPaths::for_invocation(&left.join("bin/refine")).unwrap();
        let right_paths = RefineCheckoutPaths::for_invocation(&right.join("bin/refine")).unwrap();
        assert_ne!(left_paths.checkout, right_paths.checkout);
        assert_ne!(left_paths.binary, right_paths.binary);
        assert_ne!(
            left_paths.port_runtime_root(4557),
            right_paths.port_runtime_root(4557)
        );
        assert!(
            left_paths
                .resolve_runtime_compatibility(&right_paths.runtime_root)
                .is_err()
        );

        fs::remove_dir_all(left).unwrap();
        fs::remove_dir_all(right).unwrap();
    }

    #[test]
    fn one_runtime_accepts_the_owning_debug_or_installed_executable_only() {
        let root = temp_root("two-launch-modes");
        common_shape(&root);
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/refine"), "debug\n").unwrap();
        fs::write(root.join("bin/refine"), "installed\n").unwrap();
        let paths = RefineCheckoutPaths::from_product_home(&root).unwrap();

        assert_eq!(
            paths
                .require_owned_executable(&root.join("target/debug/refine"))
                .unwrap(),
            root.canonicalize().unwrap().join("target/debug/refine")
        );
        assert_eq!(
            paths
                .require_owned_executable(&root.join("bin/refine"))
                .unwrap(),
            root.canonicalize().unwrap().join("bin/refine")
        );

        let other = temp_root("foreign-launch-mode");
        common_shape(&other);
        fs::create_dir(other.join(".git")).unwrap();
        fs::create_dir_all(other.join("target/debug")).unwrap();
        fs::write(other.join("target/debug/refine"), "foreign\n").unwrap();
        let error = paths
            .require_owned_executable(&other.join("target/debug/refine"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("belongs to product home"), "{error}");

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other).unwrap();
    }
}
