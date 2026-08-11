fn main() {
    // Embed the git commit so runtime logs prove which code is running
    // (a stale binary is indistinguishable from an unfixed bug).
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ARISTIDE_COMMIT={hash}");
    // .git/HEAD alone is NOT enough: it holds "ref: refs/heads/main" and
    // never changes on commit — the stamp would go stale, which defeats
    // proving what code is running. Track the refs themselves too.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
}
