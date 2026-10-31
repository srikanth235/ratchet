//! ratchet-candidate-launcher: the setuid-root launcher the evaluator harness uses INSIDE a bench container to start a
//! candidate process as the unprivileged candidate user. Port of `infra/images/scripts/ratchet-candidate-ctr.c`; the binary is
//! `src/bin/ratchet-candidate-launcher.rs` and this module is its logic, so that the decisions can be unit tested.
//!
//! ```text
//! /ratchet/candidate [NAME=value ...] command [args ...]
//! ```
//!
//! Why it exists: the container image is the repo's own, so it has no sudo and no way to switch users. The harness (trusted
//! code from bench.ref) runs as the holdout uid and owns the holdout file (mode 0600); the candidate's processes (its server)
//! must run as ANOTHER uid so they can neither read the holdout nor signal / ptrace the harness. This binary is static (it
//! works in any image of the same architecture), root-owned and setuid, installed in a root-only directory on the bench host
//! (/usr/local/libexec/ratchet, mode 0700) and bind-mounted read-only into the confirm and bench harness containers only.
//!
//! What it allows is read from /ratchet/launcher.conf, a root-owned file the bench script writes before each harness
//! container starts and also bind-mounts read-only: `<caller uid> <target uid> <target gid>`. It refuses to do anything
//! unless the REAL uid of its caller is the caller uid, the file is a root-owned regular file that nobody else can write, and
//! the target is neither root nor the caller. The container runs with every capability dropped except SETUID and SETGID, so
//! this is the only way the harness can change uid at all.
//!
//! The candidate process gets: no supplementary groups (`setgroups(&[])`), the target uid/gid everywhere, no_new_privs
//! (PR_SET_NO_NEW_PRIVS, so no setuid binary inside the image can give anything back), a clean environment (PATH, HOME,
//! TMPDIR, USER, LOGNAME, LANG and the NAME=value arguments; never RATCHET_TRACE or AWS_*), the checkout as working
//! directory and only stdin / stdout / stderr as open files.
//!
//! Exit status: 70 internal / unexpected error, 77 not allowed, 78 refused argument, 127 command not found (as `env`), 126
//! found but not executable.
//!
//! Differences from the C program (all in the safe direction): a uid or gid in the configuration must fit in 32 bits (the C
//! one truncated a larger number after checking it) and may not carry a sign; the command is looked up in the PATH the
//! launcher built, as `execvp` did after `environ = envp`.

use std::ffi::{CString, OsStr, OsString};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use nix::errno::Errno;
use nix::unistd::{self, Gid, Uid};

/// Where the harness container finds the root-owned configuration.
pub const CONF_PATH: &str = "/ratchet/launcher.conf";
/// `USER` / `LOGNAME` of the candidate.
pub const CANDIDATE_NAME: &str = "ratchet-bench";
/// `PATH` when neither `RATCHET_CANDIDATE_PATH` nor `PATH` is set.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub const EXIT_INTERNAL: i32 = 70;
pub const EXIT_NOT_ALLOWED: i32 = 77;
pub const EXIT_REFUSED: i32 = 78;
pub const EXIT_NOT_EXECUTABLE: i32 = 126;
pub const EXIT_NOT_FOUND: i32 = 127;

/// Why the launcher stops. Always printed as `ratchet-candidate: <message>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherError {
    pub code: i32,
    pub message: String,
}

impl LauncherError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        LauncherError {
            code,
            message: message.into(),
        }
    }

    fn not_allowed(message: &str) -> Self {
        Self::new(EXIT_NOT_ALLOWED, message)
    }

    fn refused(message: impl Into<String>) -> Self {
        Self::new(EXIT_REFUSED, message)
    }

    fn internal(message: &str) -> Self {
        Self::new(EXIT_INTERNAL, message)
    }
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ratchet-candidate: {}", self.message)
    }
}

/// `<caller uid> <target uid> <target gid>`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LauncherConf {
    pub caller: u32,
    pub uid: u32,
    pub gid: u32,
}

/// `sscanf("%lu %lu %lu %c")` with exactly three conversions: three unsigned numbers separated by whitespace and nothing but
/// whitespace after the third.
pub fn parse_conf(text: &str) -> Result<LauncherConf, LauncherError> {
    let malformed = || LauncherError::not_allowed("malformed launcher configuration");
    // C string semantics: the text ends at the first NUL
    let text = text.split('\0').next().unwrap_or("");
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r');
    let mut rest = text;
    let mut nums = [0u32; 3];
    for n in &mut nums {
        rest = rest.trim_start_matches(is_space);
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if end == 0 {
            return Err(malformed());
        }
        *n = rest[..end].parse::<u32>().map_err(|_| malformed())?;
        rest = &rest[end..];
    }
    if !rest.trim_start_matches(is_space).is_empty() {
        return Err(malformed()); // a fourth field: `%c` matched
    }
    Ok(LauncherConf {
        caller: nums[0],
        uid: nums[1],
        gid: nums[2],
    })
}

/// Step 1: who may call, and who the candidate becomes: a root-owned file nobody else can write.
pub fn read_conf(path: &Path) -> Result<LauncherConf, LauncherError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| {
            LauncherError::not_allowed(
                "no launcher configuration (this launcher only works inside a bench container)",
            )
        })?;
    let md = file.metadata().map_err(|_| {
        LauncherError::not_allowed("the launcher configuration is not a root-owned regular file")
    })?;
    if !md.is_file() || md.uid() != 0 || md.mode() & 0o022 != 0 {
        return Err(LauncherError::not_allowed(
            "the launcher configuration is not a root-owned regular file",
        ));
    }
    let mut buf = Vec::new();
    // one read of at most 127 bytes, like the C program's fixed buffer
    let got = file.by_ref().take(127).read_to_end(&mut buf).unwrap_or(0);
    if got == 0 {
        return Err(LauncherError::not_allowed("empty launcher configuration"));
    }
    parse_conf(&String::from_utf8_lossy(&buf))
}

/// The real uid of the caller must be the configured caller, and the target neither root nor the caller.
pub fn check_caller(conf: &LauncherConf, real_uid: u32) -> Result<(), LauncherError> {
    if real_uid != conf.caller {
        return Err(LauncherError::not_allowed(
            "not allowed: the caller is not the evaluator user",
        ));
    }
    if conf.uid == 0 || conf.gid == 0 || conf.uid == conf.caller {
        return Err(LauncherError::not_allowed(
            "refusing to run candidate code as root or as the evaluator user",
        ));
    }
    Ok(())
}

/// `[A-Za-z_][A-Za-z0-9_]*` over the bytes before the `=`.
pub fn valid_name(s: &[u8]) -> bool {
    match s.split_first() {
        None => false,
        Some((first, rest)) => {
            (first.is_ascii_alphabetic() || *first == b'_')
                && rest.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        }
    }
}

/// The environment of the candidate, the command and its arguments (`argv[1..]` of the launcher).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub dir: OsString,
    /// `NAME=value` entries, in the order they are passed.
    pub env: Vec<OsString>,
    /// The `PATH` the command is looked up in.
    pub path: OsString,
    pub command: Vec<OsString>,
}

fn kv(k: &str, v: &OsStr) -> OsString {
    let mut s = OsString::from(k);
    s.push("=");
    s.push(v);
    s
}

/// Step 2: the environment the candidate gets, built while we still know the harness's values. `getenv` reads the harness's
/// environment.
pub fn build_plan(
    args: &[OsString],
    getenv: impl Fn(&str) -> Option<OsString>,
) -> Result<Plan, LauncherError> {
    let need = |name: &str| -> Result<OsString, LauncherError> {
        match getenv(name) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(LauncherError::refused(format!("{name} is not set"))),
        }
    };
    let dir = need("RATCHET_CANDIDATE_DIR")?;
    let home = need("RATCHET_CANDIDATE_HOME")?;
    let tmp = need("RATCHET_CANDIDATE_TMP")?;
    let path = getenv("RATCHET_CANDIDATE_PATH")
        .filter(|p| !p.is_empty())
        .or_else(|| getenv("PATH").filter(|p| !p.is_empty()))
        .unwrap_or_else(|| OsString::from(DEFAULT_PATH));

    let mut env: Vec<OsString> = vec![
        kv("PATH", &path),
        kv("HOME", &home),
        kv("TMPDIR", &tmp),
        kv("USER", OsStr::new(CANDIDATE_NAME)),
        kv("LOGNAME", OsStr::new(CANDIDATE_NAME)),
        kv("LANG", OsStr::new("C.UTF-8")),
    ];
    let mut i = 0;
    while i < args.len() {
        let bytes = args[i].as_bytes();
        let Some(eq) = bytes.iter().position(|b| *b == b'=') else {
            break;
        };
        let name = &bytes[..eq];
        if !valid_name(name) {
            break;
        }
        if name == b"RATCHET_TRACE" || bytes.starts_with(b"AWS_") {
            return Err(LauncherError::refused(format!(
                "refusing to pass {} to candidate code",
                String::from_utf8_lossy(name)
            )));
        }
        env.push(args[i].clone());
        i += 1;
    }
    if i >= args.len() {
        return Err(LauncherError::refused(
            "usage: ratchet-candidate [NAME=value ...] command [args ...]",
        ));
    }
    Ok(Plan {
        dir,
        env,
        path,
        command: args[i..].to_vec(),
    })
}

/// Step 3: become the candidate for good: groups, gid, uid (real, effective, saved), and no way back.
fn drop_privileges(conf: &LauncherConf) -> Result<(), LauncherError> {
    let (uid, gid) = (Uid::from_raw(conf.uid), Gid::from_raw(conf.gid));
    unistd::setgroups(&[]).map_err(|_| LauncherError::internal("setgroups failed"))?;
    unistd::setresgid(gid, gid, gid).map_err(|_| LauncherError::internal("setresgid failed"))?;
    unistd::setresuid(uid, uid, uid).map_err(|_| LauncherError::internal("setresuid failed"))?;
    if unistd::getuid() != uid
        || unistd::geteuid() != uid
        || unistd::getgid() != gid
        || unistd::getegid() != gid
    {
        return Err(LauncherError::internal(
            "the identity switch did not take effect",
        ));
    }
    if unistd::setuid(Uid::from_raw(0)).is_ok() || unistd::seteuid(Uid::from_raw(0)).is_ok() {
        return Err(LauncherError::internal("the process could regain root"));
    }
    nix::sys::prctl::set_no_new_privs()
        .map_err(|_| LauncherError::internal("cannot set no_new_privs"))?;
    Ok(())
}

/// Step 4: nothing inherited but the standard streams.
fn close_inherited_fds() {
    let max = unistd::sysconf(unistd::SysconfVar::OPEN_MAX)
        .ok()
        .flatten()
        .filter(|m| *m >= 0)
        .map_or(65536, |m| m.min(65536));
    for fd in 3..i32::try_from(max).unwrap_or(65536) {
        let _ = unistd::close(fd);
    }
}

/// `execvp` with the PATH the launcher built: a command with a slash is run as is, otherwise the first PATH entry holding an
/// executable one wins. Returns the exit status for a failure (126 / 127).
fn exec_in_path(plan: &Plan) -> LauncherError {
    let cmd = &plan.command[0];
    let argv: Vec<CString> = plan
        .command
        .iter()
        .filter_map(|a| CString::new(a.as_bytes()).ok())
        .collect();
    let envp: Vec<CString> = plan
        .env
        .iter()
        .filter_map(|e| CString::new(e.as_bytes()).ok())
        .collect();
    if argv.len() != plan.command.len() {
        return LauncherError::refused("cannot run a command with a NUL byte");
    }
    let mut last = Errno::ENOENT;
    let mut candidates: Vec<Vec<u8>> = Vec::new();
    if cmd.as_bytes().contains(&b'/') {
        candidates.push(cmd.as_bytes().to_vec());
    } else {
        for dir in plan.path.as_bytes().split(|b| *b == b':') {
            let mut p = if dir.is_empty() {
                b".".to_vec()
            } else {
                dir.to_vec()
            };
            p.push(b'/');
            p.extend_from_slice(cmd.as_bytes());
            candidates.push(p);
        }
    }
    let mut saw_access = false;
    for cand in candidates {
        let Ok(c) = CString::new(cand) else { continue };
        let err = match unistd::execve(&c, &argv, &envp) {
            Err(e) => e,
            Ok(never) => match never {},
        };
        if err == Errno::ENOEXEC {
            // a script without a #! line: run by the shell, as execvp does
            let sh = CString::new("/bin/sh").unwrap_or_default();
            let mut sh_argv = vec![sh.clone(), c.clone()];
            sh_argv.extend(argv.iter().skip(1).cloned());
            let e2 = match unistd::execve(&sh, &sh_argv, &envp) {
                Err(e) => e,
                Ok(never) => match never {},
            };
            last = e2;
            break;
        }
        match err {
            Errno::EACCES => saw_access = true,
            Errno::ENOENT
            | Errno::ENOTDIR
            | Errno::ESTALE
            | Errno::ENAMETOOLONG
            | Errno::ENODEV
            | Errno::ETIMEDOUT => {}
            other => {
                last = other;
                break;
            }
        }
    }
    if saw_access && last == Errno::ENOENT {
        last = Errno::EACCES;
    }
    LauncherError::new(
        if last == Errno::ENOENT {
            EXIT_NOT_FOUND
        } else {
            EXIT_NOT_EXECUTABLE
        },
        format!(
            "cannot run {}: {}",
            String::from_utf8_lossy(cmd.as_bytes()),
            last.desc()
        ),
    )
}

/// The whole program, with the configuration path as a parameter (the binary passes [`CONF_PATH`]; tests pass their own).
/// Never returns on success: the process becomes the candidate's command.
pub fn run(conf_path: &Path, args: Vec<OsString>) -> LauncherError {
    let conf = match read_conf(conf_path) {
        Ok(c) => c,
        Err(e) => return e,
    };
    if let Err(e) = check_caller(&conf, unistd::getuid().as_raw()) {
        return e;
    }
    let plan = match build_plan(&args, |name| std::env::var_os(name)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = drop_privileges(&conf) {
        return e;
    }
    close_inherited_fds();
    if let Err(e) = unistd::chdir(Path::new(&plan.dir)) {
        return LauncherError::refused(format!(
            "cannot enter {}: {}",
            String::from_utf8_lossy(plan.dir.as_bytes()),
            e.desc()
        ));
    }
    exec_in_path(&plan)
}

/// Entry point of the binary: `exit` with the failure (printed on stderr).
pub fn main_with(conf_path: &Path) -> ! {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let err = run(conf_path, args);
    eprintln!("{err}");
    std::process::exit(err.code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let owned: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |name| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    const HARNESS: [(&str, &str); 3] = [
        ("RATCHET_CANDIDATE_DIR", "/work/code"),
        ("RATCHET_CANDIDATE_HOME", "/work/home"),
        ("RATCHET_CANDIDATE_TMP", "/work/tmp"),
    ];

    #[test]
    fn conf_is_three_numbers_and_nothing_else() {
        assert_eq!(
            parse_conf("65534 65533 65533\n"),
            Ok(LauncherConf {
                caller: 65534,
                uid: 65533,
                gid: 65533
            })
        );
        assert_eq!(parse_conf("1002 1001 1001").map(|c| c.caller), Ok(1002));
        for bad in [
            "nonsense\n",
            "65534 65533\n",
            "65534 65533 65533 extra\n",
            "",
            "-1 2 3",
            "1 2 3x",
            "99999999999 2 3",
            "1,2,3",
        ] {
            let e = parse_conf(bad).expect_err(bad);
            assert_eq!(e.code, EXIT_NOT_ALLOWED, "{bad:?}");
        }
    }

    #[test]
    fn only_the_configured_caller_may_run_and_never_towards_root_or_itself() {
        let conf = LauncherConf {
            caller: 1002,
            uid: 1001,
            gid: 1001,
        };
        assert!(check_caller(&conf, 1002).is_ok());
        assert_eq!(check_caller(&conf, 0).unwrap_err().code, EXIT_NOT_ALLOWED); // root is not the caller either
        assert_eq!(
            check_caller(&conf, 65000).unwrap_err().code,
            EXIT_NOT_ALLOWED
        );
        for bad in [
            LauncherConf {
                caller: 1002,
                uid: 0,
                gid: 0,
            },
            LauncherConf {
                caller: 1002,
                uid: 1002,
                gid: 1002,
            },
            LauncherConf {
                caller: 1002,
                uid: 1001,
                gid: 0,
            },
        ] {
            assert_eq!(check_caller(&bad, 1002).unwrap_err().code, EXIT_NOT_ALLOWED);
        }
    }

    #[test]
    fn names_are_identifiers() {
        assert!(valid_name(b"FOO"));
        assert!(valid_name(b"_x1"));
        assert!(!valid_name(b""));
        assert!(!valid_name(b"1X"));
        assert!(!valid_name(b"A-B"));
    }

    #[test]
    fn the_environment_is_built_from_scratch() {
        let args: Vec<OsString> = ["FOO=bar", "sh", "-c", "X=1 id"].map(os).to_vec();
        let mut vars = HARNESS.to_vec();
        vars.extend([
            ("PATH", "/usr/bin"),
            ("RATCHET_TRACE", "secret"),
            ("AWS_SECRET_ACCESS_KEY", "x"),
            ("OTHER", "1"),
        ]);
        let plan = build_plan(&args, env_of(&vars)).expect("plan");
        assert_eq!(plan.dir, os("/work/code"));
        assert_eq!(
            plan.env,
            [
                "PATH=/usr/bin",
                "HOME=/work/home",
                "TMPDIR=/work/tmp",
                "USER=ratchet-bench",
                "LOGNAME=ratchet-bench",
                "LANG=C.UTF-8",
                "FOO=bar"
            ]
            .map(os)
            .to_vec()
        );
        assert_eq!(plan.command, ["sh", "-c", "X=1 id"].map(os).to_vec());
        for leak in ["RATCHET_TRACE", "AWS_SECRET", "OTHER"] {
            assert!(
                plan.env.iter().all(|e| !e.to_string_lossy().contains(leak)),
                "{leak}"
            );
        }
    }

    #[test]
    fn path_comes_from_the_candidate_path_then_path_then_the_default() {
        let args = vec![os("true")];
        let with = |extra: &[(&str, &str)]| {
            let mut v = HARNESS.to_vec();
            v.extend_from_slice(extra);
            build_plan(&args, env_of(&v)).expect("plan").path
        };
        assert_eq!(
            with(&[("RATCHET_CANDIDATE_PATH", "/a"), ("PATH", "/b")]),
            os("/a")
        );
        assert_eq!(
            with(&[("RATCHET_CANDIDATE_PATH", ""), ("PATH", "/b")]),
            os("/b")
        );
        assert_eq!(with(&[]), os(DEFAULT_PATH));
    }

    #[test]
    fn the_holdout_and_credentials_cannot_be_passed_as_arguments() {
        for a in ["RATCHET_TRACE=/x", "AWS_SECRET_ACCESS_KEY=x", "AWS_=y"] {
            let e = build_plan(&[os(a), os("true")], env_of(&HARNESS)).expect_err(a);
            assert_eq!(e.code, EXIT_REFUSED);
            assert!(e.message.contains("refusing to pass"), "{}", e.message);
        }
        // a name that merely starts like one of them is fine
        assert!(build_plan(&[os("RATCHET_TRACEX=1"), os("true")], env_of(&HARNESS)).is_ok());
    }

    #[test]
    fn missing_directory_variables_and_missing_command_are_refused() {
        for missing in [
            "RATCHET_CANDIDATE_DIR",
            "RATCHET_CANDIDATE_HOME",
            "RATCHET_CANDIDATE_TMP",
        ] {
            let vars: Vec<(&str, &str)> = HARNESS
                .iter()
                .filter(|(k, _)| *k != missing)
                .copied()
                .collect();
            let e = build_plan(&[os("true")], env_of(&vars)).expect_err(missing);
            assert_eq!(e.code, EXIT_REFUSED);
            assert_eq!(e.message, format!("{missing} is not set"));
        }
        let e = build_plan(&[os("FOO=bar")], env_of(&HARNESS)).expect_err("usage");
        assert_eq!(e.code, EXIT_REFUSED);
        assert!(e.message.starts_with("usage:"));
        // the first word that is not NAME=value starts the command, even if a later one looks like an assignment
        let plan =
            build_plan(&[os("FOO=1"), os("env"), os("BAR=2")], env_of(&HARNESS)).expect("plan");
        assert_eq!(plan.command, [os("env"), os("BAR=2")]);
    }

    #[test]
    fn this_file_states_what_the_launcher_refuses() {
        let text = include_str!("launcher.rs");
        for w in [
            "setuid",
            "launcher.conf",
            "no_new_privs",
            "RATCHET_TRACE",
            "AWS_",
            "read-only",
            "PR_SET_NO_NEW_PRIVS",
            "setgroups(&[])",
        ] {
            assert!(text.contains(w), "{w}");
        }
        assert!(text.contains("/ratchet/launcher.conf"));
    }
}
