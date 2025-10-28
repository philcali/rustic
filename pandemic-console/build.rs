use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=web/");

    // Build the web frontend
    let output = Command::new("npm")
        .args(["run", "build"])
        .current_dir("web")
        .output()
        .expect("Failed to run npm build");

    if !output.status.success() {
        panic!(
            "npm build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!("Web frontend built successfully");
}
