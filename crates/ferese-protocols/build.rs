fn main() {
    println!("cargo::rerun-if-changed=../../protocols/ferese-effects-v1.xml");
    println!("cargo::rerun-if-changed=../../protocols/ferese-shell-v1.xml");
}
