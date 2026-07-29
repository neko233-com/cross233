use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = Path::new(&manifest_dir).join("..").join("web").join("dist");
    let dst = Path::new(&manifest_dir).join("webroot");
    if src.exists() {
        let _ = fs::remove_dir_all(&dst);
        copy_dir(&src, &dst).ok();
    } else {
        // Source-only/server-only installs may not have Node.js available.
        // RustEmbed still requires the folder to exist at compile time, so
        // provide a small operational page instead of failing the Rust build.
        fs::create_dir_all(&dst).expect("create fallback webroot");
        let fallback = dst.join("index.html");
        if !fallback.exists() {
            fs::write(
                fallback,
                "<!doctype html><meta charset=\"utf-8\"><title>cross233</title>\
                 <h1>cross233-server is running</h1>\
                 <p>Build <code>web/</code> before Rust to include the full dashboard.</p>",
            )
            .expect("write fallback dashboard");
        }
    }
    println!("cargo:rerun-if-changed=../web/dist");
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
