//! Trusted inside-sandbox file helper and host-side cgroup supervisor.
//! Not a user-facing execution command.
#![forbid(unsafe_code)]

fn main() {
    #[cfg(feature = "linux-execution")]
    switchyard_core::linux_helper_main();
    #[cfg(not(feature = "linux-execution"))]
    {
        eprintln!(
            "switchyard-execution-helper is not part of the default adapter; build it with --features linux-execution"
        );
        std::process::exit(1);
    }
}
