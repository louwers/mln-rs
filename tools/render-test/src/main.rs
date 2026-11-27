fn main() {
    mln_headless::log_to_stderr();
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(render_test::run_render_tests(&args, Some(render_test::map::factory(render_test::registry()))));
}
