fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    mixrelay_desktop_shell::run(&arguments);
}
