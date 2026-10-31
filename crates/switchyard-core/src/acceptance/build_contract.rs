use super::*;

#[test]
fn ci_pins_the_rust_core_job_without_replacing_npm_verify() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();
    let workflow = std::fs::read_to_string(root.join(".github/workflows/verify.yml")).unwrap();
    assert!(toolchain.contains("1.99.1"));
    assert!(workflow.contains("name: rust-core"));
    assert!(workflow.contains("runs-on: ubuntu-24.04"));
    assert!(workflow.contains("toolchain: 1.99.1"));
    assert!(workflow.contains("npm run verify"));
    assert!(workflow.contains("working-directory: adw_sdlc"));
    assert!(workflow.contains("20.19.0"));
}

#[test]
fn core_manifest_has_no_model_or_forge_client() {
    let manifest = include_str!("../../Cargo.toml");
    for banned in ["reqwest", "octocrab", "git2", "libgit2", "reqwest"] {
        assert!(
            !manifest.contains(banned),
            "{banned} must not be a dependency"
        );
    }
}
