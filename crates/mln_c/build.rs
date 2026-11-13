//! Links the browser build's module with a stack the size of the threads'
//! that other builds parse on (`mln_util::thread::STACK_SIZE`): JSON and the
//! expressions built from it recurse as deep as their input nests, and the
//! browser's instance parses on its one thread (`src/parse_thread.rs`).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_FAMILY").is_ok_and(|family| family.split(',').any(|f| f == "wasm")) {
        println!("cargo::rustc-link-arg-cdylib=-zstack-size=2097152");
    }
}
