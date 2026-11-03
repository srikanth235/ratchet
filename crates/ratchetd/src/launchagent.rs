//! The LaunchAgent: a plist that starts `ratchetd` at login and keeps it alive, so runs survive quitting the app.
//!
//! `ratchetd launchagent install|uninstall|print`. Tests never call `launchctl`: it sits behind [`Launchctl`] and
//! the tests use [`NoLaunchctl`]. The shipped app can instead embed the output of `print --bundle-program` in
//! `Contents/Library/LaunchAgents/` and register it with `SMAppService` (the architecture doc's plan).

use std::io;
use std::path::{Path, PathBuf};

/// Label of the LaunchAgent.
pub const LABEL: &str = "com.ratchet.ratchetd";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAgentSpec {
    pub label: String,
    /// Absolute path of the `ratchetd` binary. Ignored when `bundle_program` is set.
    pub program: PathBuf,
    /// Relative to the app bundle (`Contents/MacOS/ratchetd`): for an `SMAppService` plist.
    pub bundle_program: Option<String>,
    pub args: Vec<String>,
    /// `AssociatedBundleIdentifiers`: the app that owns this agent, so Login Items shows the app's name.
    pub associated_bundle_ids: Vec<String>,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    /// `(name, value)` pairs, e.g. `RATCHET_HOME`.
    pub env: Vec<(String, String)>,
}

impl LaunchAgentSpec {
    pub fn new(program: impl Into<PathBuf>, log_dir: impl AsRef<Path>) -> Self {
        let log_dir = log_dir.as_ref();
        Self {
            label: LABEL.to_owned(),
            program: program.into(),
            bundle_program: None,
            args: vec!["serve".into()],
            associated_bundle_ids: Vec::new(),
            stdout_path: log_dir.join(crate::paths::LOG_FILE),
            stderr_path: log_dir.join(crate::paths::LOG_FILE),
            env: Vec::new(),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The plist XML.
pub fn plist(spec: &LaunchAgentSpec) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n");
    s.push_str("<plist version=\"1.0\">\n<dict>\n");
    s.push_str(&format!(
        "\t<key>Label</key>\n\t<string>{}</string>\n",
        esc(&spec.label)
    ));
    match &spec.bundle_program {
        Some(p) => {
            s.push_str(&format!(
                "\t<key>BundleProgram</key>\n\t<string>{}</string>\n",
                esc(p)
            ));
            s.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
            s.push_str(&format!("\t\t<string>{}</string>\n", esc(p)));
        }
        None => {
            s.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
            s.push_str(&format!(
                "\t\t<string>{}</string>\n",
                esc(&spec.program.display().to_string())
            ));
        }
    }
    for a in &spec.args {
        s.push_str(&format!("\t\t<string>{}</string>\n", esc(a)));
    }
    s.push_str("\t</array>\n");
    s.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    // restart after a crash, not after a clean exit (`launchctl bootout`, or the user quitting it)
    s.push_str("\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n");
    if !spec.associated_bundle_ids.is_empty() {
        s.push_str("\t<key>AssociatedBundleIdentifiers</key>\n\t<array>\n");
        for id in &spec.associated_bundle_ids {
            s.push_str(&format!("\t\t<string>{}</string>\n", esc(id)));
        }
        s.push_str("\t</array>\n");
    }
    s.push_str("\t<key>ProcessType</key>\n\t<string>Background</string>\n");
    s.push_str("\t<key>ThrottleInterval</key>\n\t<integer>5</integer>\n");
    // An app-bundle plist is written at build time, on another machine: it cannot name a log path under the user's
    // home. The daemon logs to its support directory itself (`serve --log-to-support-dir`).
    if spec.bundle_program.is_none() {
        s.push_str(&format!(
            "\t<key>StandardOutPath</key>\n\t<string>{}</string>\n",
            esc(&spec.stdout_path.display().to_string())
        ));
        s.push_str(&format!(
            "\t<key>StandardErrorPath</key>\n\t<string>{}</string>\n",
            esc(&spec.stderr_path.display().to_string())
        ));
    }
    if !spec.env.is_empty() {
        s.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
        for (k, v) in &spec.env {
            s.push_str(&format!(
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                esc(k),
                esc(v)
            ));
        }
        s.push_str("\t</dict>\n");
    }
    s.push_str("</dict>\n</plist>\n");
    s
}

/// `~/Library/LaunchAgents/<label>.plist`
pub fn plist_path(home: &Path, label: &str) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{label}.plist"))
}

/// What loads and unloads a LaunchAgent.
pub trait Launchctl {
    fn load(&self, plist: &Path, label: &str) -> io::Result<()>;
    fn unload(&self, label: &str) -> io::Result<()>;
}

/// Does nothing (tests, `--no-load`, non-macOS).
pub struct NoLaunchctl;

impl Launchctl for NoLaunchctl {
    fn load(&self, _: &Path, _: &str) -> io::Result<()> {
        Ok(())
    }
    fn unload(&self, _: &str) -> io::Result<()> {
        Ok(())
    }
}

/// `launchctl bootstrap gui/<uid> <plist>` and `launchctl bootout gui/<uid>/<label>`.
pub struct SystemLaunchctl;

fn domain() -> String {
    // SAFETY: getuid has no preconditions.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn run(args: &[&str]) -> io::Result<()> {
    let out = std::process::Command::new("launchctl")
        .args(args)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "launchctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

impl Launchctl for SystemLaunchctl {
    fn load(&self, plist: &Path, label: &str) -> io::Result<()> {
        // replace a loaded copy (an upgrade)
        let _ = run(&["bootout", &format!("{}/{label}", domain())]);
        run(&["bootstrap", &domain(), &plist.display().to_string()])
    }

    fn unload(&self, label: &str) -> io::Result<()> {
        run(&["bootout", &format!("{}/{label}", domain())])
    }
}

/// Write the plist (atomically) and load it. Returns the plist path.
pub fn install(spec: &LaunchAgentSpec, home: &Path, ctl: &dyn Launchctl) -> io::Result<PathBuf> {
    let path = plist_path(home, &spec.label);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Some(dir) = spec.stdout_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("plist.tmp");
    std::fs::write(&tmp, plist(spec))?;
    std::fs::rename(&tmp, &path)?;
    ctl.load(&path, &spec.label)?;
    Ok(path)
}

/// Unload the agent and remove its plist. Returns whether a plist was there.
pub fn uninstall(label: &str, home: &Path, ctl: &dyn Launchctl) -> io::Result<bool> {
    let path = plist_path(home, label);
    // an agent that was never loaded is not an error
    let _ = ctl.unload(label);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);

    impl Launchctl for Recorder {
        fn load(&self, plist: &Path, label: &str) -> io::Result<()> {
            self.0
                .borrow_mut()
                .push(format!("load {label} {}", plist.display()));
            Ok(())
        }
        fn unload(&self, label: &str) -> io::Result<()> {
            self.0.borrow_mut().push(format!("unload {label}"));
            Ok(())
        }
    }

    fn spec(dir: &Path) -> LaunchAgentSpec {
        let mut s = LaunchAgentSpec::new("/Applications/Ratchet.app/Contents/MacOS/ratchetd", dir);
        s.env.push(("RATCHET_HOME".into(), "/tmp/a&b".into()));
        s
    }

    #[test]
    fn the_plist_names_the_program_and_keeps_it_alive() {
        let p = plist(&spec(Path::new("/logs")));
        assert!(p.contains("<key>Label</key>\n\t<string>com.ratchet.ratchetd</string>"));
        assert!(p.contains("<string>/Applications/Ratchet.app/Contents/MacOS/ratchetd</string>"));
        assert!(p.contains("<string>serve</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(p.contains("/logs/ratchetd.log"));
        assert!(p.contains("/tmp/a&amp;b"), "values are XML-escaped");
        assert!(!p.contains("BundleProgram"));
    }

    #[test]
    fn an_app_bundle_plist_uses_bundle_program() {
        let mut s = spec(Path::new("/logs"));
        s.bundle_program = Some("Contents/MacOS/ratchetd".into());
        let p = plist(&s);
        assert!(p.contains("<key>BundleProgram</key>\n\t<string>Contents/MacOS/ratchetd</string>"));
    }

    #[test]
    fn an_app_bundle_plist_has_no_machine_specific_log_path_and_names_its_app() {
        let mut s = spec(Path::new("/builder/home/logs"));
        s.bundle_program = Some("Contents/Helpers/ratchetd".into());
        s.args = vec![
            "serve".into(),
            "--mode".into(),
            "real".into(),
            "--log-to-support-dir".into(),
        ];
        s.associated_bundle_ids = vec!["dev.ratchet.app".into()];
        let p = plist(&s);
        assert!(!p.contains("StandardOutPath"));
        assert!(!p.contains("/builder/home"));
        assert!(p.contains("<string>--log-to-support-dir</string>"));
        assert!(p.contains("<key>AssociatedBundleIdentifiers</key>\n\t<array>\n\t\t<string>dev.ratchet.app</string>"));
    }

    #[test]
    fn install_writes_the_plist_and_loads_it_uninstall_removes_it() {
        let home = tempfile::tempdir().unwrap();
        let ctl = Recorder::default();
        let s = spec(&home.path().join("logs"));
        let path = install(&s, home.path(), &ctl).unwrap();
        assert_eq!(
            path,
            home.path()
                .join("Library/LaunchAgents/com.ratchet.ratchetd.plist")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), plist(&s));
        assert!(home.path().join("logs").is_dir());
        // upgrade in place
        install(&s, home.path(), &ctl).unwrap();
        assert!(uninstall(LABEL, home.path(), &ctl).unwrap());
        assert!(!path.exists());
        assert!(!uninstall(LABEL, home.path(), &ctl).unwrap());
        let calls = ctl.0.borrow();
        assert_eq!(calls.iter().filter(|c| c.starts_with("load")).count(), 2);
        assert_eq!(calls.iter().filter(|c| c.starts_with("unload")).count(), 2);
    }

    #[test]
    fn no_launchctl_is_a_no_op() {
        let home = tempfile::tempdir().unwrap();
        install(&spec(home.path()), home.path(), &NoLaunchctl).unwrap();
        assert!(uninstall(LABEL, home.path(), &NoLaunchctl).unwrap());
    }
}
