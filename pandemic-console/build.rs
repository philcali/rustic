use std::process::Command;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=web/");

    // Check if dist directory already exists (pre-built)
    if Path::new("web/dist").exists() {
        println!("Using pre-built web frontend");
        return;
    }

    // Try to build the web frontend
    match Command::new("npm")
        .args(["run", "build"])
        .current_dir("web")
        .output()
    {
        Ok(output) => {
            if !output.status.success() {
                eprintln!("npm build failed: {}", String::from_utf8_lossy(&output.stderr));
                eprintln!("Using fallback web assets");
                create_fallback_dist();
            } else {
                println!("Web frontend built successfully");
            }
        }
        Err(_) => {
            println!("npm not available, using fallback web assets");
            create_fallback_dist();
        }
    }
}

fn create_fallback_dist() {
    std::fs::create_dir_all("web/dist").unwrap();
    std::fs::write(
        "web/dist/index.html",
        r#"<!DOCTYPE html>
<html><head><title>Pandemic Console</title></head>
<body><h1>Pandemic Console</h1><p>Web interface not available</p></body></html>"#
    ).unwrap();
}
