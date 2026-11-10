// Rebuild when the web UI is rebuilt, so the embedded copy is never stale.
fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");
}
