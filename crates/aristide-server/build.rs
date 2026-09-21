fn main() {
    embed_console();
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

// One UI in both the standalone server and Tauri shell. Embed only public
// runtime assets, never tests or arbitrary files from the repository.
fn embed_console() {
    use std::{fs, path::Path};
    fn collect(root: &Path, dir: &Path, entries: &mut Vec<(String, String, String)>) {
        for entry in fs::read_dir(dir).expect("read console assets") {
            let path = entry.expect("console asset").path();
            if path.is_dir() {
                collect(root, &path, entries);
                continue;
            }
            let relative = path.strip_prefix(root).expect("asset beneath root");
            let name = relative.to_string_lossy().replace('\\', "/");
            if name.ends_with(".test.js") {
                continue;
            }
            let mime = match path.extension().and_then(|ext| ext.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("woff2") => "font/woff2",
                Some("txt") => "text/plain; charset=utf-8",
                _ => continue,
            };
            entries.push((
                format!("/{name}"),
                path.to_string_lossy().into_owned(),
                mime.into(),
            ));
        }
    }
    let root = Path::new("../aristide-console/ui")
        .canonicalize()
        .expect("console UI directory");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut entries = Vec::new();
    collect(&root, &root, &mut entries);
    entries.sort();
    let mut source = String::from(
        "pub fn asset(path: &str) -> Option<(&'static [u8], &'static str)> { match path {\n",
    );
    for (url, path, mime) in entries {
        let route = if url == "/index.html" {
            format!("\"/\" | {url:?}")
        } else {
            format!("{url:?}")
        };
        source.push_str(&format!(
            "{route} => Some((include_bytes!({path:?}), {mime:?})),\n"
        ));
    }
    source.push_str("_ => None, } }\n");
    let out = std::env::var_os("OUT_DIR").expect("Cargo output directory");
    fs::write(Path::new(&out).join("console_assets.rs"), source).expect("write console asset map");
}
