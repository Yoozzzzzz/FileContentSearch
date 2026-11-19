#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--extract-worker")) {
        file_content_search::worker::run_child();
        return;
    }
    file_content_search::run();
}
