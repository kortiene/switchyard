//! Startup barrier: this process never migrates itself into a cgroup.
use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::Command;

pub fn confined_pi_launcher_main() {
    if let Err(error) = run() {
        eprintln!("switchyard-bridge-launcher: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let expected_parent = args
        .next()
        .ok_or("missing owner PID")?
        .parse::<i32>()
        .map_err(|error| error.to_string())?;
    let parent = rustix::process::getppid();
    // Delivered when the spawning thread exits. The session keeps that
    // supervisor alive until stop. The signal survives exec; bubblewrap
    // `--die-with-parent` sets SIGKILL again in the sandbox parent.
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))
        .map_err(|error| error.to_string())?;
    if expected_parent <= 0
        || parent.is_none_or(|p| p.as_raw_nonzero().get() != expected_parent)
        || parent != rustix::process::getppid()
    {
        return Err("owner died before the startup barrier".into());
    }
    let leaf = args.next().ok_or("missing cgroup")?;
    if args.next().as_deref() != Some("--") {
        return Err("missing command separator".into());
    }
    let argv: Vec<String> = args.collect();
    if argv.is_empty() {
        return Err("missing fixed sandbox command".into());
    }
    // File is unbuffered: never consume protocol bytes that must survive exec.
    let mut release = [0_u8; 3];
    fs::File::open("/proc/self/fd/0")
        .map_err(|error| error.to_string())?
        .read_exact(&mut release)
        .map_err(|error| error.to_string())?;
    if release != *b"GO\n" {
        return Err("owner did not release the bridge".into());
    }
    let members = fs::read_to_string(std::path::Path::new(&leaf).join("cgroup.procs"))
        .map_err(|error| error.to_string())?;
    if !members
        .split_whitespace()
        .any(|s| s == std::process::id().to_string())
    {
        return Err("owner did not place the launcher in its cgroup".into());
    }
    Err(Command::new(&argv[0]).args(&argv[1..]).exec().to_string())
}
